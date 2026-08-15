//! Shared package layer for ZIP-based formats (DOCX, PPTX, ODF, EPUB):
//! limited archive access, namespace-aware XML, typed relationships, and
//! OPC/EPUB target resolution.

pub mod archive;
pub mod limits;
pub mod path;
pub mod relationships;
pub mod xml;

pub use archive::Package;
