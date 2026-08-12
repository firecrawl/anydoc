//! Python bindings for anydoc.

use std::path::PathBuf;

use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList};

mod document;

create_exception!(
    anydoc,
    ConvertError,
    PyException,
    "Meaningful conversion was impossible. Catch this to handle every kind of \
     failure, or one of the subclasses below to single one out. An unreadable \
     file raises `OSError` instead."
);

create_exception!(
    anydoc,
    UnsupportedError,
    ConvertError,
    "The format is unknown, or cannot be converted at all: a scanned or \
     image-only PDF needs OCR, which anydoc does not do."
);

create_exception!(
    anydoc,
    MalformedError,
    ConvertError,
    "The document is structurally unusable: no meaningful content could be \
     extracted. `part` names the package part or stream at fault, and is \
     `None` when no single part is."
);

create_exception!(
    anydoc,
    EncryptedError,
    ConvertError,
    "The document is encrypted or password-protected."
);

create_exception!(
    anydoc,
    ResourceLimitError,
    ConvertError,
    "A fixed safety limit was crossed: decompression, nesting depth, node \
     count, repeat expansion, or retained asset bytes. `limit` names it."
);

create_exception!(
    anydoc,
    MissingPartError,
    ConvertError,
    "A part required for any meaningful output is absent. `part` names it."
);

/// Format names, as the extension that identifies each format. Container
/// variants that share a parser (`.docm`, `.xlsm`, `.ppsx`, ...) map onto
/// these via `format_from_bytes` or `format_from_extension`.
const FORMATS: [(&str, anydoc::Format); 12] = [
    ("doc", anydoc::Format::Doc),
    ("docx", anydoc::Format::Docx),
    ("odt", anydoc::Format::Odt),
    ("pdf", anydoc::Format::Pdf),
    ("ppt", anydoc::Format::Ppt),
    ("pptx", anydoc::Format::Pptx),
    ("rtf", anydoc::Format::Rtf),
    ("epub", anydoc::Format::Epub),
    ("xlsx", anydoc::Format::Excel),
    ("ods", anydoc::Format::Ods),
    ("odp", anydoc::Format::Odp),
    ("csv", anydoc::Format::Csv),
];

fn parse_format(name: &str) -> PyResult<anydoc::Format> {
    FORMATS.iter().find(|(n, _)| *n == name).map(|(_, format)| *format).ok_or_else(|| {
        let names: Vec<&str> = FORMATS.iter().map(|(n, _)| *n).collect();
        PyValueError::new_err(format!(
            "unknown format {name:?}; expected one of {}",
            names.join(", ")
        ))
    })
}

fn format_name(format: anydoc::Format) -> &'static str {
    FORMATS
        .iter()
        .find(|(_, f)| *f == format)
        .map(|(name, _)| *name)
        .expect("every format is named")
}

/// Raise the subclass that names the failure, carrying the part or limit at
/// fault where the variant knows one. An unreadable file raises the `OSError`
/// subclass any other read of it would.
fn convert_error(py: Python<'_>, error: anydoc::ConvertError) -> PyErr {
    let error = match error {
        anydoc::ConvertError::Io(e) => return e.into(),
        other => other,
    };
    let message = error.to_string();
    // A variant added later raises the base class until it is named here.
    let raised = match &error {
        anydoc::ConvertError::Unsupported(_) => UnsupportedError::new_err(message),
        anydoc::ConvertError::Malformed { .. } => MalformedError::new_err(message),
        anydoc::ConvertError::Encrypted => EncryptedError::new_err(message),
        anydoc::ConvertError::ResourceLimit { .. } => ResourceLimitError::new_err(message),
        anydoc::ConvertError::MissingPart { .. } => MissingPartError::new_err(message),
        _ => ConvertError::new_err(message),
    };
    let detail = match &error {
        anydoc::ConvertError::Malformed { part, .. } => {
            raised.value(py).setattr("part", part.as_deref())
        }
        anydoc::ConvertError::ResourceLimit { limit, .. } => {
            raised.value(py).setattr("limit", *limit)
        }
        anydoc::ConvertError::MissingPart { part } => {
            raised.value(py).setattr("part", part.as_str())
        }
        _ => Ok(()),
    };
    detail.err().unwrap_or(raised)
}

/// Detect the format from the content itself: the signature and identity each
/// container specification designates (PDF header, RTF open group, OLE stream
/// names, ZIP package mimetype/content types). Plain-text formats (CSV) carry
/// no signature and return `None`; so does anything unrecognized.
#[pyfunction]
fn format_from_bytes(data: Vec<u8>) -> Option<&'static str> {
    anydoc::Format::from_bytes(&data).map(format_name)
}

/// The format an extension names, with or without a leading dot.
#[pyfunction]
fn format_from_extension(extension: &str) -> Option<&'static str> {
    anydoc::Format::from_extension(extension.trim_start_matches('.')).map(format_name)
}

/// The format a path's extension names.
#[pyfunction]
fn format_from_path(path: PathBuf) -> Option<&'static str> {
    anydoc::Format::from_path(&path).map(format_name)
}

/// Convert a document file to Markdown. The format is detected from the file
/// content; the extension is the fallback for signature-less formats (CSV)
/// and unrecognizable containers.
#[pyfunction]
fn to_markdown(py: Python<'_>, path: PathBuf) -> PyResult<String> {
    py.detach(|| anydoc::to_markdown(&path)).map_err(|e| convert_error(py, e))
}

/// Convert an in-memory document to Markdown. Without a format, it is
/// detected from the content, which signature-less formats (CSV) have to name
/// explicitly.
#[pyfunction]
#[pyo3(signature = (data, format=None))]
fn to_markdown_bytes(py: Python<'_>, data: Vec<u8>, format: Option<&str>) -> PyResult<String> {
    let format = format.map(parse_format).transpose()?;
    py.detach(|| anydoc::to_markdown_bytes(&data, format)).map_err(|e| convert_error(py, e))
}

/// One positioned PDF image encoded as PNG.
#[pyclass(frozen, get_all, module = "anydoc")]
struct PdfImage {
    filename: String,
    /// One-based source page number.
    page: u32,
    /// Source placement as (x, y, width, height) in PDF points.
    bbox: (f32, f32, f32, f32),
    width: u32,
    height: u32,
    data: Py<PyBytes>,
    warnings: Vec<String>,
}

/// PDF Markdown and the images referenced by it.
#[pyclass(frozen, get_all, module = "anydoc")]
struct PdfConversion {
    markdown: String,
    /// list[PdfImage]
    images: Py<PyList>,
    page_count: u32,
    pages_needing_ocr: Vec<u32>,
    /// list[PdfOcrReasons]
    ocr_reasons_by_page: Py<PyList>,
    pages_with_tables: Vec<u32>,
    pages_with_columns: Vec<u32>,
    is_complex_layout: bool,
    has_encoding_issues: bool,
}

/// OCR diagnostics for one PDF page.
#[pyclass(frozen, get_all, module = "anydoc")]
struct PdfOcrReasons {
    page: u32,
    reasons: Vec<String>,
}

/// Convert PDF bytes to Markdown and return the PNG data for each positioned
/// image. A `PdfImage.filename` is the exact target used in `markdown`.
#[pyfunction]
fn pdf_to_markdown_with_images(py: Python<'_>, data: Vec<u8>) -> PyResult<PdfConversion> {
    let conversion = py
        .detach(|| anydoc::pdf_to_markdown_with_images(&data))
        .map_err(|e| convert_error(py, e))?;
    let images = conversion
        .images
        .into_iter()
        .map(|image| {
            let [x, y, width, height] = image.bbox;
            Py::new(
                py,
                PdfImage {
                    filename: image.filename,
                    page: image.page,
                    bbox: (x, y, width, height),
                    width: image.width,
                    height: image.height,
                    data: PyBytes::new(py, &image.data).unbind(),
                    warnings: image.warnings,
                },
            )
        })
        .collect::<PyResult<Vec<_>>>()?;
    let ocr_reasons = conversion
        .ocr_reasons_by_page
        .into_iter()
        .map(|reason| Py::new(py, PdfOcrReasons { page: reason.page, reasons: reason.reasons }))
        .collect::<PyResult<Vec<_>>>()?;
    Ok(PdfConversion {
        markdown: conversion.markdown,
        images: PyList::new(py, images)?.unbind(),
        page_count: conversion.page_count,
        pages_needing_ocr: conversion.pages_needing_ocr,
        ocr_reasons_by_page: PyList::new(py, ocr_reasons)?.unbind(),
        pages_with_tables: conversion.pages_with_tables,
        pages_with_columns: conversion.pages_with_columns,
        is_complex_layout: conversion.is_complex_layout,
        has_encoding_issues: conversion.has_encoding_issues,
    })
}

/// Parse an in-memory document into the document model, which also carries
/// the embedded assets. Without a format, it is detected from the content.
///
/// Unsupported for `pdf`: PDF conversion produces Markdown directly and has
/// no document-model form; use `to_markdown_bytes`.
#[pyfunction]
#[pyo3(signature = (data, format=None))]
fn to_document(
    py: Python<'_>,
    data: Vec<u8>,
    format: Option<&str>,
) -> PyResult<document::Document> {
    let format = format.map(parse_format).transpose()?;
    let parsed =
        py.detach(|| anydoc::to_document(&data, format)).map_err(|e| convert_error(py, e))?;
    document::document(py, parsed)
}

/// Convert documents to GitHub-Flavored Markdown.
#[pymodule]
fn _anydoc(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(format_from_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(format_from_extension, m)?)?;
    m.add_function(wrap_pyfunction!(format_from_path, m)?)?;
    m.add_function(wrap_pyfunction!(to_markdown, m)?)?;
    m.add_function(wrap_pyfunction!(to_markdown_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(pdf_to_markdown_with_images, m)?)?;
    m.add_function(wrap_pyfunction!(to_document, m)?)?;
    m.add_class::<document::Asset>()?;
    m.add_class::<document::Block>()?;
    m.add_class::<document::Cell>()?;
    m.add_class::<document::CellSlot>()?;
    m.add_class::<document::Document>()?;
    m.add_class::<document::ImageSource>()?;
    m.add_class::<document::Inline>()?;
    m.add_class::<document::LinkTarget>()?;
    m.add_class::<document::List>()?;
    m.add_class::<document::ListItem>()?;
    m.add_class::<document::Note>()?;
    m.add_class::<document::Style>()?;
    m.add_class::<document::Table>()?;
    m.add_class::<PdfConversion>()?;
    m.add_class::<PdfImage>()?;
    m.add_class::<PdfOcrReasons>()?;
    m.add("ConvertError", m.py().get_type::<ConvertError>())?;
    m.add("EncryptedError", m.py().get_type::<EncryptedError>())?;
    m.add("MalformedError", m.py().get_type::<MalformedError>())?;
    m.add("MissingPartError", m.py().get_type::<MissingPartError>())?;
    m.add("ResourceLimitError", m.py().get_type::<ResourceLimitError>())?;
    m.add("UnsupportedError", m.py().get_type::<UnsupportedError>())?;
    Ok(())
}
