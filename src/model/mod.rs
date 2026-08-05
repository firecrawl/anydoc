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

/// A parsed document: its body, its notes, and the bytes of everything it
/// embedded.
#[derive(Debug, Clone, Default)]
pub struct Document {
    /// Body content in reading order.
    pub blocks: Vec<Block>,
    /// Note bodies, in the order the document defines them. Text refers to
    /// them by id through [`Inline::NoteRef`].
    pub notes: Vec<Note>,
    /// Every embedded asset, indexed by [`AssetId`].
    pub assets: Vec<Asset>,
}

/// One structural heading in document reading order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineEntry {
    /// Source outline depth, 1-based.
    pub level: u8,
    /// Heading text with inline styling and links flattened away.
    pub text: String,
    /// Stable source anchor, when the heading carries one.
    pub anchor: Option<AnchorId>,
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

impl Document {
    /// Return structural headings in body reading order. Headings in list
    /// items and block quotes are included because they remain body content;
    /// table-cell headings and note bodies are excluded because they are
    /// layout or auxiliary content rather than document structure.
    pub fn outline(&self) -> Vec<OutlineEntry> {
        let mut entries = Vec::new();
        collect_outline(&self.blocks, &mut entries);
        entries
    }

    /// Remove redundant whole-heading character styles from the resolved
    /// model. This is intentionally performed before format consumers see the
    /// document, so every renderer gets the same heading semantics.
    pub(crate) fn normalize_heading_styles(&mut self) {
        normalize_blocks(&mut self.blocks);
        for note in &mut self.notes {
            normalize_blocks(&mut note.blocks);
        }
    }
}

fn collect_outline(blocks: &[Block], entries: &mut Vec<OutlineEntry>) {
    for block in blocks {
        match block {
            Block::Heading { level, anchor, content } => entries.push(OutlineEntry {
                level: *level,
                text: inlines_to_plain_text(content),
                anchor: anchor.clone(),
            }),
            Block::List(list) => {
                for item in &list.items {
                    collect_outline(&item.blocks, entries);
                }
            }
            Block::BlockQuote(nested) => collect_outline(nested, entries),
            Block::Paragraph(_) | Block::Table(_) | Block::CodeBlock { .. } | Block::Rule => {}
        }
    }
}

fn normalize_blocks(blocks: &mut [Block]) {
    for block in blocks {
        match block {
            Block::Heading { content, .. } => inline::strip_uniform_style(content),
            Block::List(list) => {
                for item in &mut list.items {
                    normalize_blocks(&mut item.blocks);
                }
            }
            Block::Table(table) => {
                for row in &mut table.grid {
                    for slot in row {
                        if let table::CellSlot::Origin(cell) = slot {
                            normalize_blocks(&mut cell.blocks);
                        }
                    }
                }
            }
            Block::BlockQuote(nested) => normalize_blocks(nested),
            Block::Paragraph(_) | Block::CodeBlock { .. } | Block::Rule => {}
        }
    }
}

/// Where the source document places a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    /// Placed at the foot of the page that references it.
    Footnote,
    /// Collected at the end of the document or section.
    Endnote,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heading(level: u8, text: &str, anchor: Option<&str>) -> Block {
        Block::Heading {
            level,
            anchor: anchor.map(str::to_string),
            content: vec![Inline::plain(text)],
        }
    }

    #[test]
    fn outline_preserves_order_anchors_and_document_boundaries() {
        let table = Table::from_rows(
            vec![vec![Cell::new(vec![heading(4, "Cell", None)])]],
            0,
            TableKind::Layout,
        );
        let document = Document {
            blocks: vec![
                heading(1, "First", None),
                Block::BlockQuote(vec![heading(3, "Quoted", Some("quoted"))]),
                Block::List(List {
                    marker: MarkerKind::Bullet,
                    start: 1,
                    items: vec![ListItem {
                        blocks: vec![heading(2, "Listed", None)],
                        ..ListItem::default()
                    }],
                }),
                Block::Table(table),
            ],
            notes: vec![Note {
                id: "n1".into(),
                kind: NoteKind::Footnote,
                blocks: vec![heading(5, "Note", None)],
            }],
            assets: Vec::new(),
        };

        assert_eq!(
            document.outline(),
            vec![
                OutlineEntry { level: 1, text: "First".into(), anchor: None },
                OutlineEntry { level: 3, text: "Quoted".into(), anchor: Some("quoted".into()) },
                OutlineEntry { level: 2, text: "Listed".into(), anchor: None },
            ]
        );
    }
}
