//! C ABI exposed to .NET, consumed by csbindgen (`build.rs`) and marshalled by
//! the C# wrapper in `../src`.
//!
//! Inputs are UTF-8 byte slices and results are a small by-value struct whose
//! payload is an owned buffer the caller frees with [`anydoc_free_result`].
//! This keeps every crossing a plain `{ptr,len}` pair, so the ABI is identical
//! on all six supported targets and never relies on `repr(C)` layout sharing a
//! struct with C#.

use std::ffi::{c_char, CString};
use std::panic;
use std::path::Path;
use std::slice;

use anydoc::ConvertError;

mod serialize;

/// Format codes shared with the C# `Format` enum (ordinal match). `-1` means
/// "unspecified": let detection name it.
const DOC: i32 = 0;
const DOCX: i32 = 1;
const ODT: i32 = 2;
const PDF: i32 = 3;
const PPT: i32 = 4;
const PPTX: i32 = 5;
const RTF: i32 = 6;
const EPUB: i32 = 7;
const EXCEL: i32 = 8;
const ODS: i32 = 9;
const ODP: i32 = 10;
const CSV: i32 = 11;

fn format_code(format: anydoc::Format) -> i32 {
    match format {
        anydoc::Format::Doc => DOC,
        anydoc::Format::Docx => DOCX,
        anydoc::Format::Odt => ODT,
        anydoc::Format::Pdf => PDF,
        anydoc::Format::Ppt => PPT,
        anydoc::Format::Pptx => PPTX,
        anydoc::Format::Rtf => RTF,
        anydoc::Format::Epub => EPUB,
        anydoc::Format::Excel => EXCEL,
        anydoc::Format::Ods => ODS,
        anydoc::Format::Odp => ODP,
        anydoc::Format::Csv => CSV,
    }
}

fn format_from_code(code: i32) -> Option<anydoc::Format> {
    Some(match code {
        DOC => anydoc::Format::Doc,
        DOCX => anydoc::Format::Docx,
        ODT => anydoc::Format::Odt,
        PDF => anydoc::Format::Pdf,
        PPT => anydoc::Format::Ppt,
        PPTX => anydoc::Format::Pptx,
        RTF => anydoc::Format::Rtf,
        EPUB => anydoc::Format::Epub,
        EXCEL => anydoc::Format::Excel,
        ODS => anydoc::Format::Ods,
        ODP => anydoc::Format::Odp,
        CSV => anydoc::Format::Csv,
        _ => return None,
    })
}

/// An owned byte buffer; `ptr` is null and `len` zero when empty.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ByteBuffer {
    pub ptr: *mut u8,
    pub len: usize,
    pub capacity: usize,
}

impl ByteBuffer {
    fn empty() -> Self {
        Self { ptr: std::ptr::null_mut(), len: 0, capacity: 0 }
    }

    fn from_vec(bytes: Vec<u8>) -> Self {
        let mut bytes = std::mem::ManuallyDrop::new(bytes);
        Self { ptr: bytes.as_mut_ptr(), len: bytes.len(), capacity: bytes.capacity() }
    }
}

/// The outcome of a conversion. `ok` selects between the payload and the error
/// fields; the caller hands the whole struct back to [`anydoc_free_result`].
#[repr(C)]
#[derive(Clone, Copy)]
pub struct AnyResult {
    pub ok: bool,
    /// Machine-readable kind (`error.code()`), null when `ok`.
    pub error_code: *mut c_char,
    /// Human-readable message, null when `ok`.
    pub error_message: *mut c_char,
    /// Markdown, or the document model as JSON, when `ok`.
    pub data: ByteBuffer,
}

impl AnyResult {
    fn ok(bytes: Vec<u8>) -> Self {
        Self { ok: true, error_code: std::ptr::null_mut(), error_message: std::ptr::null_mut(), data: ByteBuffer::from_vec(bytes) }
    }

    fn err(error: ConvertError) -> Self {
        let code = CString::new(error.code()).unwrap();
        let message = CString::new(error.to_string()).unwrap();
        Self {
            ok: false,
            error_code: code.into_raw(),
            error_message: message.into_raw(),
            data: ByteBuffer::empty(),
        }
    }
}

unsafe fn utf8<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 { &[] } else { unsafe { slice::from_raw_parts(ptr, len) } }
}

/// `format_from_bytes`: detect the format from content.
#[unsafe(no_mangle)]
pub extern "C" fn anydoc_format_from_bytes(bytes: *const u8, len: usize) -> i32 {
    let bytes = unsafe { utf8(bytes, len) };
    anydoc::Format::from_bytes(bytes).map(format_code).unwrap_or(-1)
}

/// `format_from_extension`: the format a (possibly dot-prefixed) extension names.
#[unsafe(no_mangle)]
pub extern "C" fn anydoc_format_from_extension(bytes: *const u8, len: usize) -> i32 {
    let bytes = unsafe { utf8(bytes, len) };
    let ext = std::str::from_utf8(bytes).unwrap_or_default();
    anydoc::Format::from_extension(ext.trim_start_matches('.')).map(format_code).unwrap_or(-1)
}

/// `format_from_path`: the format a path's extension names.
#[unsafe(no_mangle)]
pub extern "C" fn anydoc_format_from_path(bytes: *const u8, len: usize) -> i32 {
    let bytes = unsafe { utf8(bytes, len) };
    let path = std::str::from_utf8(bytes).unwrap_or_default();
    anydoc::Format::from_path(Path::new(path)).map(format_code).unwrap_or(-1)
}

/// `to_markdown(path)`: convert a file, detecting the format from its content.
#[unsafe(no_mangle)]
pub extern "C" fn anydoc_to_markdown_path(path: *const u8, len: usize, result: *mut AnyResult) {
    let path = unsafe { utf8(path, len) };
    let path = std::str::from_utf8(path).unwrap_or_default();
    let outcome = panic::catch_unwind(|| anydoc::to_markdown(Path::new(path)).map(|s| s.into_bytes()));
    fill(result, outcome);
}

/// `to_markdown_bytes`: convert in-memory bytes; `format` of `-1` detects it.
#[unsafe(no_mangle)]
pub extern "C" fn anydoc_to_markdown_bytes(
    bytes: *const u8,
    len: usize,
    format: i32,
    result: *mut AnyResult,
) {
    let bytes = unsafe { utf8(bytes, len) };
    let outcome = panic::catch_unwind(|| {
        anydoc::to_markdown_bytes(bytes, format_from_code(format)).map(|s| s.into_bytes())
    });
    fill(result, outcome);
}

/// `to_document`: parse in-memory bytes into the document model, returned as
/// JSON. Unsupported for PDF, like the other bindings.
#[unsafe(no_mangle)]
pub extern "C" fn anydoc_to_document(
    bytes: *const u8,
    len: usize,
    format: i32,
    result: *mut AnyResult,
) {
    let bytes = unsafe { utf8(bytes, len) };
    let outcome = panic::catch_unwind(|| {
        anydoc::to_document(bytes, format_from_code(format)).map(|document| {
            serde_json::to_vec(&serialize::document(document)).expect("model serializes")
        })
    });
    fill(result, outcome);
}

/// Release everything `*result` holds: the error strings and the data buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn anydoc_free_result(result: *mut AnyResult) {
    if result.is_null() {
        return;
    }
    unsafe {
        let result = &mut *result;
        if !result.error_code.is_null() {
            drop(CString::from_raw(result.error_code));
        }
        if !result.error_message.is_null() {
            drop(CString::from_raw(result.error_message));
        }
        if !result.data.ptr.is_null() {
            drop(Vec::from_raw_parts(
                result.data.ptr,
                result.data.len,
                result.data.capacity,
            ));
        }
    }
}

fn fill(
    result: *mut AnyResult,
    outcome: Result<Result<Vec<u8>, ConvertError>, Box<dyn std::any::Any + Send>>,
) {
    if result.is_null() {
        return;
    }
    let value = match outcome {
        Ok(Ok(bytes)) => {
            unsafe { *result = AnyResult::ok(bytes) };
            return;
        }
        Ok(Err(error)) => AnyResult::err(error),
        Err(_) => AnyResult::err(ConvertError::Unsupported("internal conversion panic".into())),
    };
    unsafe { *result = value };
}
