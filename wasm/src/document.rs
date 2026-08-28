//! The document model, serialized to plain JS objects. The shapes mirror the
//! Node bindings; the TypeScript definitions live in `typescript.rs`.

use serde::Serialize;

use anydoc::model;

#[derive(Serialize)]
pub struct Document {
    pub blocks: Vec<Block>,
    /// Footnote and endnote bodies, referenced from text by a `noteRef`
    /// inline.
    pub notes: Vec<Note>,
    pub assets: Vec<Asset>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BlockKind {
    Heading,
    Paragraph,
    List,
    Table,
    BlockQuote,
    CodeBlock,
    Rule,
    Math,
}

#[derive(Serialize)]
pub struct Block {
    pub kind: BlockKind,
    /// heading: 1-6.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<u32>,
    /// heading: stable anchor id when the document targets this heading.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// heading, paragraph.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<Inline>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<List>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table: Option<Table>,
    /// blockQuote.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<Block>>,
    /// codeBlock.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// codeBlock, math (LaTeX source without delimiters).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl Block {
    fn of(kind: BlockKind) -> Block {
        Block {
            kind,
            level: None,
            anchor: None,
            content: None,
            list: None,
            table: None,
            blocks: None,
            lang: None,
            text: None,
        }
    }
}

impl From<model::Block> for Block {
    fn from(block: model::Block) -> Self {
        match block {
            model::Block::Heading { level, anchor, content } => Block {
                level: Some(level.into()),
                anchor,
                content: Some(inlines(content)),
                ..Block::of(BlockKind::Heading)
            },
            model::Block::Paragraph(content) => {
                Block { content: Some(inlines(content)), ..Block::of(BlockKind::Paragraph) }
            }
            model::Block::List(list) => {
                Block { list: Some(list.into()), ..Block::of(BlockKind::List) }
            }
            model::Block::Table(table) => {
                Block { table: Some(table.into()), ..Block::of(BlockKind::Table) }
            }
            model::Block::BlockQuote(inner) => {
                Block { blocks: Some(blocks(inner)), ..Block::of(BlockKind::BlockQuote) }
            }
            model::Block::CodeBlock { lang, text } => {
                Block { lang, text: Some(text), ..Block::of(BlockKind::CodeBlock) }
            }
            model::Block::Rule => Block::of(BlockKind::Rule),
            model::Block::Math(tex) => Block { text: Some(tex), ..Block::of(BlockKind::Math) },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InlineKind {
    Text,
    Link,
    Image,
    /// Zero-width marker for an internal link target at this position.
    Anchor,
    NoteRef,
    LineBreak,
    /// An inline formula.
    Math,
    /// A checkbox control.
    Checkbox,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inline {
    pub kind: InlineKind,
    /// text; math (LaTeX source without delimiters).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<Style>,
    /// link.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<Inline>>,
    /// link.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<LinkTarget>,
    /// image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
    /// image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ImageSource>,
    /// anchor: the anchor id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// noteRef: the id of the note in `Document.notes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note_id: Option<String>,
    /// checkbox: its state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<bool>,
}

impl Inline {
    fn of(kind: InlineKind) -> Inline {
        Inline {
            kind,
            text: None,
            style: None,
            content: None,
            target: None,
            alt: None,
            source: None,
            anchor: None,
            note_id: None,
            checked: None,
        }
    }
}

impl From<model::Inline> for Inline {
    fn from(inline: model::Inline) -> Self {
        match inline {
            model::Inline::Text { text, style } => Inline {
                text: Some(text),
                style: Some(style.into()),
                ..Inline::of(InlineKind::Text)
            },
            model::Inline::Link { content, target } => Inline {
                content: Some(inlines(content)),
                target: Some(target.into()),
                ..Inline::of(InlineKind::Link)
            },
            model::Inline::Image { alt, source } => Inline {
                alt: Some(alt),
                source: Some(source.into()),
                ..Inline::of(InlineKind::Image)
            },
            model::Inline::Anchor(id) => {
                Inline { anchor: Some(id), ..Inline::of(InlineKind::Anchor) }
            }
            model::Inline::NoteRef(id) => {
                Inline { note_id: Some(id), ..Inline::of(InlineKind::NoteRef) }
            }
            model::Inline::LineBreak => Inline::of(InlineKind::LineBreak),
            model::Inline::Math(tex) => Inline { text: Some(tex), ..Inline::of(InlineKind::Math) },
            model::Inline::Checkbox(checked) => {
                Inline { checked: Some(checked), ..Inline::of(InlineKind::Checkbox) }
            }
        }
    }
}

/// Fully resolved character style.
#[derive(Serialize)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

impl From<model::Style> for Style {
    fn from(style: model::Style) -> Self {
        Style { bold: style.bold, italic: style.italic, strike: style.strike, code: style.code }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkTargetKind {
    /// Absolute URL with a scheme.
    External,
    /// Scheme-less relative reference, preserved as written.
    Relative,
    /// Internal target: a heading anchor or an `anchor` inline.
    Anchor,
}

#[derive(Serialize)]
pub struct LinkTarget {
    pub kind: LinkTargetKind,
    /// The URL, relative reference, or anchor id.
    pub value: String,
}

impl From<model::LinkTarget> for LinkTarget {
    fn from(target: model::LinkTarget) -> Self {
        let (kind, value) = match target {
            model::LinkTarget::External(v) => (LinkTargetKind::External, v),
            model::LinkTarget::Relative(v) => (LinkTargetKind::Relative, v),
            model::LinkTarget::Anchor(v) => (LinkTargetKind::Anchor, v),
        };
        LinkTarget { kind, value }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageSourceKind {
    /// Absolute URL with a scheme.
    External,
    /// Embedded image, carried in `Document.assets`.
    Asset,
    /// No usable source: the image's part is missing or unreadable and it has
    /// no URL. Only the alt text remains.
    Unavailable,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageSource {
    pub kind: ImageSourceKind,
    /// external.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// asset: index into `Document.assets`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<u32>,
}

impl From<model::ImageSource> for ImageSource {
    fn from(source: model::ImageSource) -> Self {
        match source {
            model::ImageSource::External(url) => {
                ImageSource { kind: ImageSourceKind::External, url: Some(url), asset_id: None }
            }
            model::ImageSource::Asset(id) => {
                ImageSource { kind: ImageSourceKind::Asset, url: None, asset_id: Some(id.0 as u32) }
            }
            model::ImageSource::Unavailable => {
                ImageSource { kind: ImageSourceKind::Unavailable, url: None, asset_id: None }
            }
        }
    }
}

/// The marker family a list uses in the source document.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum MarkerKind {
    Bullet,
    Decimal,
    LowerAlpha,
    UpperAlpha,
    LowerRoman,
    UpperRoman,
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

#[derive(Serialize)]
pub struct List {
    pub marker: MarkerKind,
    /// Ordinal the first item counts from.
    pub start: u32,
    pub items: Vec<ListItem>,
}

impl From<model::List> for List {
    fn from(list: model::List) -> Self {
        List {
            marker: list.marker.into(),
            start: list.start.min(u32::MAX.into()) as u32,
            items: list.items.into_iter().map(ListItem::from).collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListItem {
    pub blocks: Vec<Block>,
    /// Literal marker text that overrides the list marker when the source
    /// number text cannot be reproduced from the marker and position alone
    /// (composite number text such as `1-a)`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub marker_label: Option<String>,
}

impl From<model::ListItem> for ListItem {
    fn from(item: model::ListItem) -> Self {
        ListItem { blocks: blocks(item.blocks), marker_label: item.marker_label }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TableKind {
    /// A real data table.
    Data,
    /// Layout scaffolding (text boxes, positioning tables).
    Layout,
}

/// Canonical table grid: every logical grid position appears exactly once.
/// Content and spans live on the origin slot, and each position a span covers
/// holds a `covered` slot pointing back at that origin.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub grid: Vec<Vec<CellSlot>>,
    /// Number of leading rows that are header rows (0 = no header).
    pub header_rows: u32,
    pub kind: TableKind,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CellSlotKind {
    Origin,
    Covered,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CellSlot {
    pub kind: CellSlotKind,
    /// origin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cell: Option<Cell>,
    /// covered: row of the origin this position belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_row: Option<u32>,
    /// covered: column of the origin this position belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_col: Option<u32>,
}

impl From<model::CellSlot> for CellSlot {
    fn from(slot: model::CellSlot) -> Self {
        match slot {
            model::CellSlot::Origin(cell) => CellSlot {
                kind: CellSlotKind::Origin,
                cell: Some(cell.into()),
                origin_row: None,
                origin_col: None,
            },
            model::CellSlot::Covered { origin_row, origin_col } => CellSlot {
                kind: CellSlotKind::Covered,
                cell: None,
                origin_row: Some(origin_row as u32),
                origin_col: Some(origin_col as u32),
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    pub blocks: Vec<Block>,
    pub col_span: u32,
    pub row_span: u32,
}

impl From<model::Cell> for Cell {
    fn from(cell: model::Cell) -> Self {
        Cell { blocks: blocks(cell.blocks), col_span: cell.col_span, row_span: cell.row_span }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NoteKind {
    Footnote,
    Endnote,
}

#[derive(Serialize)]
pub struct Note {
    pub id: String,
    pub kind: NoteKind,
    pub blocks: Vec<Block>,
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

/// An embedded binary asset (image, object payload). Bytes are always
/// retained, so a document stays self-contained.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    /// Index into `Document.assets`, as referenced by an image source.
    pub id: u32,
    /// MIME type, e.g. `image/png`.
    pub media_type: String,
    /// Package part or stream the asset came from, for provenance.
    pub origin_part: String,
    /// Serialized as a `Uint8Array`.
    pub data: serde_bytes::ByteBuf,
}

impl From<model::Asset> for Asset {
    fn from(asset: model::Asset) -> Self {
        Asset {
            id: asset.id.0 as u32,
            media_type: asset.media_type,
            origin_part: asset.origin_part,
            data: serde_bytes::ByteBuf::from(asset.bytes),
        }
    }
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

fn blocks(blocks: Vec<model::Block>) -> Vec<Block> {
    blocks.into_iter().map(Block::from).collect()
}

fn inlines(inlines: Vec<model::Inline>) -> Vec<Inline> {
    inlines.into_iter().map(Inline::from).collect()
}
