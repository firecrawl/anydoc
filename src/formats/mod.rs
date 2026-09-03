//! One frontend per input format; each parses bytes into the document model.

mod csv;
pub mod detect;
mod doc;
mod docx;
mod epub;
mod odf;
pub mod pdf;
mod ppt;
mod pptx;
mod rtf;
mod sheet;

use crate::Format;
use crate::error::ConvertError;
use crate::model::Document;

pub fn parse(bytes: &[u8], format: Format) -> Result<Document, ConvertError> {
    match format {
        Format::Excel => sheet::parse(bytes),
        Format::Csv => csv::parse(bytes),
        Format::Docx => docx::parse(bytes),
        Format::Odt | Format::Ods | Format::Odp => odf::parse(bytes),
        Format::Pptx => pptx::parse(bytes),
        Format::Epub => epub::parse(bytes),
        Format::Rtf => rtf::parse(bytes),
        // RTF files wearing a .doc extension are common in the wild.
        Format::Doc if bytes.starts_with(b"{\\rtf") => rtf::parse(bytes),
        Format::Doc => doc::parse(bytes),
        Format::Ppt => ppt::parse(bytes),
        // pdf-inspector produces Markdown directly; there is no document
        // model for PDFs. `to_markdown_bytes` / `to_markdown_pages_bytes`
        // route them to `pdf`.
        Format::Pdf => Err(ConvertError::Unsupported(
            "PDF converts directly to Markdown; use to_markdown, to_markdown_bytes, or to_markdown_pages".to_string(),
        )),
    }
}
