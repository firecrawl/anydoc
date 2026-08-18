//! The document model, mapped onto UniFFI types.
//!
//! Recursive ADTs (`Block`, `Inline`) stay enums so Kotlin gets sealed
//! classes. `List` is exported as `DocList` to avoid colliding with
//! `kotlin.collections.List`.

use anydoc::model;

/// A parsed document: its body, its notes, and the bytes of everything it
/// embedded.
#[derive(Debug, uniffi::Record)]
pub struct Document {
    pub blocks: Vec<Block>,
    /// Footnote and endnote bodies, referenced from text by a `note_ref`
    /// inline.
    pub notes: Vec<Note>,
    pub assets: Vec<Asset>,
}

/// One block-level piece of a document body.
#[derive(Debug, uniffi::Enum)]
pub enum Block {
    Heading {
        level: u8,
        anchor: Option<String>,
        content: Vec<Inline>,
    },
    Paragraph {
        content: Vec<Inline>,
    },
    /// Named `ListBlock` so Kotlin does not shadow `kotlin.collections.List`.
    ListBlock {
        list: DocList,
    },
    /// Named `TableBlock` so it does not shadow the `Table` record type.
    TableBlock {
        table: Table,
    },
    BlockQuote {
        blocks: Vec<Block>,
    },
    CodeBlock {
        lang: Option<String>,
        text: String,
    },
    Rule,
}

/// One span of inline content.
#[derive(Debug, uniffi::Enum)]
pub enum Inline {
    Text {
        text: String,
        style: Style,
    },
    Link {
        content: Vec<Inline>,
        target: LinkTarget,
    },
    Image {
        alt: String,
        source: ImageSource,
    },
    /// Zero-width marker for an internal link target at this position.
    Anchor {
        anchor: String,
    },
    NoteRef {
        note_id: String,
    },
    LineBreak,
}

/// Fully resolved character style.
#[derive(Debug, uniffi::Record)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

/// Where a link points.
#[derive(Debug, uniffi::Enum)]
pub enum LinkTarget {
    /// Absolute URL with a scheme.
    External { value: String },
    /// Scheme-less relative reference, preserved as written.
    Relative { value: String },
    /// Internal target: a heading anchor or an `anchor` inline.
    Anchor { value: String },
}

/// Where an image's bytes live.
#[derive(Debug, uniffi::Enum)]
pub enum ImageSource {
    External { url: String },
    Asset { asset_id: u32 },
    Unavailable,
}

/// The marker family a list uses in the source document.
#[derive(Debug, uniffi::Enum)]
pub enum MarkerKind {
    Bullet,
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
}

/// A fully resolved list. Named `DocList` in Kotlin so it does not shadow
/// `kotlin.collections.List`.
#[derive(Debug, uniffi::Record)]
pub struct DocList {
    pub marker: MarkerKind,
    /// Ordinal of the first item, from the source's own numbering.
    pub start: u64,
    pub items: Vec<ListItem>,
}

/// One item of a [`DocList`], which may hold nested blocks including further
/// lists.
#[derive(Debug, uniffi::Record)]
pub struct ListItem {
    pub blocks: Vec<Block>,
    /// Checkbox state for a task list item; `None` when the item carries no
    /// checkbox.
    pub checked: Option<bool>,
    /// Literal marker text that overrides the level marker when the source
    /// number text cannot be reproduced from `marker` + position alone.
    pub marker_label: Option<String>,
}

/// What a table is for.
#[derive(Debug, uniffi::Enum)]
pub enum TableKind {
    Data,
    Layout,
}

/// Canonical table grid: every logical grid position appears exactly once.
#[derive(Debug, uniffi::Record)]
pub struct Table {
    pub grid: Vec<Vec<CellSlot>>,
    /// Number of leading rows that are header rows (0 = no header).
    pub header_rows: u32,
    pub kind: TableKind,
}

/// One position in a [`Table`] grid: either a cell or the shadow of one.
#[derive(Debug, uniffi::Enum)]
pub enum CellSlot {
    Origin { cell: Cell },
    Covered { origin_row: u32, origin_col: u32 },
}

/// A table cell and the extent it spans.
#[derive(Debug, uniffi::Record)]
pub struct Cell {
    pub blocks: Vec<Block>,
    pub col_span: u32,
    pub row_span: u32,
}

/// Footnote or endnote body, referenced from text by [`Inline::NoteRef`].
#[derive(Debug, uniffi::Record)]
pub struct Note {
    pub id: String,
    pub kind: NoteKind,
    pub blocks: Vec<Block>,
}

/// Where the source document places a note.
#[derive(Debug, uniffi::Enum)]
pub enum NoteKind {
    Footnote,
    Endnote,
}

/// An embedded binary asset (image, object payload).
#[derive(Debug, uniffi::Record)]
pub struct Asset {
    /// Index into `Document.assets`, as referenced by an image source.
    pub id: u32,
    /// MIME type, e.g. `image/png`.
    pub media_type: String,
    /// Package part or stream the asset came from, for provenance.
    pub origin_part: String,
    pub data: Vec<u8>,
}

impl From<model::Document> for Document {
    fn from(document: model::Document) -> Self {
        Document {
            blocks: blocks(document.blocks),
            notes: document.notes.into_iter().map(Note::from).collect(),
            assets: document.assets.into_iter().map(Asset::from).collect(),
        }
    }
}

impl From<model::Block> for Block {
    fn from(block: model::Block) -> Self {
        match block {
            model::Block::Heading { level, anchor, content } => {
                Block::Heading { level, anchor, content: inlines(content) }
            }
            model::Block::Paragraph(content) => Block::Paragraph { content: inlines(content) },
            model::Block::List(list) => Block::ListBlock { list: list.into() },
            model::Block::Table(table) => Block::TableBlock { table: table.into() },
            model::Block::BlockQuote(inner) => Block::BlockQuote { blocks: blocks(inner) },
            model::Block::CodeBlock { lang, text } => Block::CodeBlock { lang, text },
            model::Block::Rule => Block::Rule,
        }
    }
}

impl From<model::Inline> for Inline {
    fn from(inline: model::Inline) -> Self {
        match inline {
            model::Inline::Text { text, style } => Inline::Text { text, style: style.into() },
            model::Inline::Link { content, target } => {
                Inline::Link { content: inlines(content), target: target.into() }
            }
            model::Inline::Image { alt, source } => Inline::Image { alt, source: source.into() },
            model::Inline::Anchor(anchor) => Inline::Anchor { anchor },
            model::Inline::NoteRef(note_id) => Inline::NoteRef { note_id },
            model::Inline::LineBreak => Inline::LineBreak,
        }
    }
}

impl From<model::Style> for Style {
    fn from(style: model::Style) -> Self {
        Style { bold: style.bold, italic: style.italic, strike: style.strike, code: style.code }
    }
}

impl From<model::LinkTarget> for LinkTarget {
    fn from(target: model::LinkTarget) -> Self {
        match target {
            model::LinkTarget::External(value) => LinkTarget::External { value },
            model::LinkTarget::Relative(value) => LinkTarget::Relative { value },
            model::LinkTarget::Anchor(value) => LinkTarget::Anchor { value },
        }
    }
}

impl From<model::ImageSource> for ImageSource {
    fn from(source: model::ImageSource) -> Self {
        match source {
            model::ImageSource::External(url) => ImageSource::External { url },
            model::ImageSource::Asset(id) => ImageSource::Asset { asset_id: id.0 as u32 },
            model::ImageSource::Unavailable => ImageSource::Unavailable,
        }
    }
}

impl From<model::MarkerKind> for MarkerKind {
    fn from(marker: model::MarkerKind) -> Self {
        match marker {
            model::MarkerKind::Bullet => MarkerKind::Bullet,
            model::MarkerKind::Decimal => MarkerKind::Decimal,
            model::MarkerKind::LowerAlpha => MarkerKind::LowerAlpha,
            model::MarkerKind::UpperAlpha => MarkerKind::UpperAlpha,
            model::MarkerKind::LowerRoman => MarkerKind::LowerRoman,
            model::MarkerKind::UpperRoman => MarkerKind::UpperRoman,
        }
    }
}

impl From<model::List> for DocList {
    fn from(list: model::List) -> Self {
        DocList {
            marker: list.marker.into(),
            start: list.start,
            items: list.items.into_iter().map(ListItem::from).collect(),
        }
    }
}

impl From<model::ListItem> for ListItem {
    fn from(item: model::ListItem) -> Self {
        ListItem {
            blocks: blocks(item.blocks),
            checked: item.checked,
            marker_label: item.marker_label,
        }
    }
}

impl From<model::Table> for Table {
    fn from(table: model::Table) -> Self {
        Table {
            grid: table
                .grid
                .into_iter()
                .map(|row| row.into_iter().map(CellSlot::from).collect())
                .collect(),
            header_rows: table.header_rows as u32,
            kind: match table.kind {
                model::TableKind::Data => TableKind::Data,
                model::TableKind::Layout => TableKind::Layout,
            },
        }
    }
}

impl From<model::CellSlot> for CellSlot {
    fn from(slot: model::CellSlot) -> Self {
        match slot {
            model::CellSlot::Origin(cell) => CellSlot::Origin { cell: cell.into() },
            model::CellSlot::Covered { origin_row, origin_col } => {
                CellSlot::Covered { origin_row: origin_row as u32, origin_col: origin_col as u32 }
            }
        }
    }
}

impl From<model::Cell> for Cell {
    fn from(cell: model::Cell) -> Self {
        Cell { blocks: blocks(cell.blocks), col_span: cell.col_span, row_span: cell.row_span }
    }
}

impl From<model::Note> for Note {
    fn from(note: model::Note) -> Self {
        Note {
            id: note.id,
            kind: match note.kind {
                model::NoteKind::Footnote => NoteKind::Footnote,
                model::NoteKind::Endnote => NoteKind::Endnote,
            },
            blocks: blocks(note.blocks),
        }
    }
}

impl From<model::Asset> for Asset {
    fn from(asset: model::Asset) -> Self {
        Asset {
            id: asset.id.0 as u32,
            media_type: asset.media_type,
            origin_part: asset.origin_part,
            data: asset.bytes,
        }
    }
}

fn blocks(blocks: Vec<model::Block>) -> Vec<Block> {
    blocks.into_iter().map(Block::from).collect()
}

fn inlines(inlines: Vec<model::Inline>) -> Vec<Inline> {
    inlines.into_iter().map(Inline::from).collect()
}
