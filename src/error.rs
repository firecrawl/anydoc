//! Typed conversion errors.
//!
//! An error means a complete conversion was impossible: the input was
//! unreadable or structurally unusable, encrypted, or crossed a fixed
//! safety/resource limit. Recoverable producer quirks never surface here -
//! they are recovered or skipped (and logged via the `log` facade) while
//! conversion continues.

use std::fmt;

/// Why a conversion could not produce a useful result.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConvertError {
    /// The format is unknown or cannot be converted at all.
    Unsupported(String),
    /// Some pages of a PDF are scanned or image-only and need OCR, which
    /// anydoc does not do. Nothing is returned for the document, so output
    /// missing those pages never passes as complete.
    NeedsOcr {
        /// 1-indexed pages that need OCR.
        pages: Vec<u32>,
        /// Pages in the document.
        page_count: u32,
    },
    /// The document is structurally unusable - no meaningful content could be
    /// extracted. `part` names the package part or stream when known.
    Malformed {
        /// Package part or stream the failure was found in, when known.
        part: Option<String>,
        /// What was wrong with it.
        detail: String,
    },
    /// The document is encrypted or password-protected.
    Encrypted,
    /// A fixed safety limit was exceeded (decompression, nesting depth, node
    /// count, repeat expansion, retained asset bytes). These are hard errors
    /// in every case; see `package::limits` for the documented defaults.
    ResourceLimit {
        /// Name of the limit that was hit.
        limit: &'static str,
        /// What exceeded it, and by how much where that is known.
        detail: String,
    },
    /// A part required for any meaningful output is missing.
    MissingPart {
        /// The part or stream that was absent.
        part: String,
    },
    /// The input could not be read.
    Io(std::io::Error),
}

impl fmt::Display for ConvertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConvertError::Unsupported(what) => write!(f, "unsupported input: {what}"),
            ConvertError::NeedsOcr { pages, page_count } => match pages.as_slice() {
                [page] => write!(f, "page {page} of {page_count} needs OCR"),
                _ if pages.len() as u32 >= *page_count => {
                    write!(f, "all {page_count} pages need OCR")
                }
                _ => write!(f, "pages {} of {page_count} need OCR", page_ranges(pages)),
            },
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
            ConvertError::Io(e) => write!(f, "io error: {e}"),
        }
    }
}

impl std::error::Error for ConvertError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConvertError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ConvertError {
    fn from(e: std::io::Error) -> Self {
        ConvertError::Io(e)
    }
}

impl ConvertError {
    /// Stable, machine-readable name for the variant: what a caller branches
    /// on, where the `Display` message carries the detail. The Node and wasm
    /// bindings publish it as `error.code`.
    pub fn code(&self) -> &'static str {
        match self {
            ConvertError::Unsupported(_) => "unsupported",
            ConvertError::NeedsOcr { .. } => "needsOcr",
            ConvertError::Malformed { .. } => "malformed",
            ConvertError::Encrypted => "encrypted",
            ConvertError::ResourceLimit { .. } => "resourceLimit",
            ConvertError::MissingPart { .. } => "missingPart",
            ConvertError::Io(_) => "io",
        }
    }

    pub(crate) fn malformed(detail: impl Into<String>) -> Self {
        ConvertError::Malformed { part: None, detail: detail.into() }
    }

    pub(crate) fn malformed_part(part: impl Into<String>, detail: impl Into<String>) -> Self {
        ConvertError::Malformed { part: Some(part.into()), detail: detail.into() }
    }

    /// True when recovery must not swallow this error: fixed safety limits
    /// hard-fail in every context, including optional parts.
    pub(crate) fn is_fatal(&self) -> bool {
        matches!(self, ConvertError::ResourceLimit { .. })
    }
}

/// `2, 5-7, 12` from ascending page numbers.
fn page_ranges(pages: &[u32]) -> String {
    let mut ranges = Vec::new();
    let mut pages = pages.iter().copied().peekable();
    while let Some(start) = pages.next() {
        let mut end = start;
        while pages.next_if_eq(&(end + 1)).is_some() {
            end += 1;
        }
        ranges.push(if end > start { format!("{start}-{end}") } else { start.to_string() });
    }
    ranges.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The CLI shows only the message, so it has to name the pages.
    #[test]
    fn needs_ocr_names_the_pages() {
        let scattered = ConvertError::NeedsOcr { pages: vec![2, 5, 6, 7, 12], page_count: 20 };
        assert_eq!(scattered.to_string(), "pages 2, 5-7, 12 of 20 need OCR");
        let all = ConvertError::NeedsOcr { pages: vec![1, 2], page_count: 2 };
        assert_eq!(all.to_string(), "all 2 pages need OCR");
    }

    /// The bindings publish these verbatim as `error.code`, so changing one
    /// breaks every caller that branches on it.
    #[test]
    fn codes_name_every_variant() {
        assert_eq!(ConvertError::Unsupported(String::new()).code(), "unsupported");
        assert_eq!(ConvertError::NeedsOcr { pages: vec![1], page_count: 1 }.code(), "needsOcr");
        assert_eq!(ConvertError::malformed("").code(), "malformed");
        assert_eq!(ConvertError::malformed_part("word/document.xml", "").code(), "malformed");
        assert_eq!(ConvertError::Encrypted.code(), "encrypted");
        let limit = ConvertError::ResourceLimit { limit: "max_entry_bytes", detail: String::new() };
        assert_eq!(limit.code(), "resourceLimit");
        assert_eq!(ConvertError::MissingPart { part: String::new() }.code(), "missingPart");
        assert_eq!(ConvertError::Io(std::io::ErrorKind::NotFound.into()).code(), "io");
    }
}
