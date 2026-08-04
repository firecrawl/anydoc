//! Information-preserving document model.
//!
//! Only fully resolved content lives here: format frontends resolve style
//! cascades, numbering, and references before constructing these types.
//! A [`Document`] is self-contained - embedded assets carry their bytes, so it
//! stays usable after the source archive is gone.

mod asset;
mod block;
mod inline;
mod link;
mod list;
mod style;
mod table;

pub use asset::{Asset, AssetId};
pub use block::Block;
pub use inline::{Inline, inlines_are_empty, inlines_to_plain_text};
pub use link::{AnchorId, ImageSource, LinkTarget};
pub use list::{List, ListItem, MarkerKind};
pub use style::Style;
pub use table::{Cell, CellSlot, Table, TableKind};

/// Frontends build grids; consumers read them off [`Table::grid`].
pub(crate) use table::GridBuilder;

/// A parsed document: its metadata, its body, its notes, and the bytes of
/// everything it embedded.
#[derive(Debug, Clone, Default)]
pub struct Document {
    /// Document-level metadata, when the source carries it (EPUB Dublin Core
    /// today). Empty for formats that expose none.
    pub meta: DocumentMeta,
    /// Body content in reading order.
    pub blocks: Vec<Block>,
    /// Note bodies, in the order the document defines them. Text refers to
    /// them by id through [`Inline::NoteRef`].
    pub notes: Vec<Note>,
    /// Every embedded asset, indexed by [`AssetId`].
    pub assets: Vec<Asset>,
}

/// Document-level metadata. Every field is optional; a source that names none
/// yields the default (all empty), which renders no output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentMeta {
    /// Document title.
    pub title: Option<String>,
    /// Authors, in source order (EPUB `dc:creator` may repeat).
    pub authors: Vec<String>,
    /// BCP-47 / ISO language tag.
    pub language: Option<String>,
    /// Publication or creation date, as the source spells it.
    pub date: Option<String>,
    /// Publisher name.
    pub publisher: Option<String>,
    /// Free-text description or summary.
    pub description: Option<String>,
}

impl DocumentMeta {
    /// Whether the source named any metadata at all.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.authors.is_empty()
            && self.language.is_none()
            && self.date.is_none()
            && self.publisher.is_none()
            && self.description.is_none()
    }
}

/// Footnote or endnote body, referenced from text by [`Inline::NoteRef`].
#[derive(Debug, Clone)]
pub struct Note {
    /// Document-scoped id the referencing [`Inline::NoteRef`] carries.
    pub id: String,
    /// Whether the source placed this note on the page or at the end.
    pub kind: NoteKind,
    /// The note's own content.
    pub blocks: Vec<Block>,
}

/// Where the source document places a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    /// Placed at the foot of the page that references it.
    Footnote,
    /// Collected at the end of the document or section.
    Endnote,
}
