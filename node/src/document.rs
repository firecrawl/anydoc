//! The document model.

use napi::bindgen_prelude::*;
use napi_derive::napi;

use anydoc::model;

#[napi(object)]
pub struct Document {
    pub blocks: Vec<Block>,
    /// Footnote and endnote bodies, referenced from text by a `noteRef`
    /// inline.
    pub notes: Vec<Note>,
    pub assets: Vec<Asset>,
}

#[napi(string_enum)]
#[allow(non_camel_case_types)]
pub enum BlockKind {
    heading,
    paragraph,
    list,
    table,
    blockQuote,
    codeBlock,
    rule,
    math,
}

#[napi(object)]
pub struct Block {
    pub kind: BlockKind,
    /// heading: 1-6.
    pub level: Option<u32>,
    /// heading: stable anchor id when the document targets this heading.
    pub anchor: Option<String>,
    /// heading, paragraph.
    pub content: Option<Vec<Inline>>,
    pub list: Option<List>,
    pub table: Option<Table>,
    /// blockQuote.
    pub blocks: Option<Vec<Block>>,
    /// codeBlock.
    pub lang: Option<String>,
    /// codeBlock, math (LaTeX source without delimiters).
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
                ..Block::of(BlockKind::heading)
            },
            model::Block::Paragraph(content) => {
                Block { content: Some(inlines(content)), ..Block::of(BlockKind::paragraph) }
            }
            model::Block::List(list) => {
                Block { list: Some(list.into()), ..Block::of(BlockKind::list) }
            }
            model::Block::Table(table) => {
                Block { table: Some(table.into()), ..Block::of(BlockKind::table) }
            }
            model::Block::BlockQuote(inner) => {
                Block { blocks: Some(blocks(inner)), ..Block::of(BlockKind::blockQuote) }
            }
            model::Block::CodeBlock { lang, text } => {
                Block { lang, text: Some(text), ..Block::of(BlockKind::codeBlock) }
            }
            model::Block::Rule => Block::of(BlockKind::rule),
            model::Block::Math(tex) => Block { text: Some(tex), ..Block::of(BlockKind::math) },
        }
    }
}

#[napi(string_enum)]
#[allow(non_camel_case_types)]
pub enum InlineKind {
    text,
    link,
    image,
    /// Zero-width marker for an internal link target at this position.
    anchor,
    noteRef,
    lineBreak,
    /// An inline formula.
    math,
}

#[napi(object)]
pub struct Inline {
    pub kind: InlineKind,
    /// text; math (LaTeX source without delimiters).
    pub text: Option<String>,
    /// text.
    pub style: Option<Style>,
    /// link.
    pub content: Option<Vec<Inline>>,
    /// link.
    pub target: Option<LinkTarget>,
    /// image.
    pub alt: Option<String>,
    /// image.
    pub source: Option<ImageSource>,
    /// anchor: the anchor id.
    pub anchor: Option<String>,
    /// noteRef: the id of the note in `Document.notes`.
    pub note_id: Option<String>,
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
        }
    }
}

impl From<model::Inline> for Inline {
    fn from(inline: model::Inline) -> Self {
        match inline {
            model::Inline::Text { text, style } => Inline {
                text: Some(text),
                style: Some(style.into()),
                ..Inline::of(InlineKind::text)
            },
            model::Inline::Link { content, target } => Inline {
                content: Some(inlines(content)),
                target: Some(target.into()),
                ..Inline::of(InlineKind::link)
            },
            model::Inline::Image { alt, source } => Inline {
                alt: Some(alt),
                source: Some(source.into()),
                ..Inline::of(InlineKind::image)
            },
            model::Inline::Anchor(id) => {
                Inline { anchor: Some(id), ..Inline::of(InlineKind::anchor) }
            }
            model::Inline::NoteRef(id) => {
                Inline { note_id: Some(id), ..Inline::of(InlineKind::noteRef) }
            }
            model::Inline::LineBreak => Inline::of(InlineKind::lineBreak),
            model::Inline::Math(tex) => Inline { text: Some(tex), ..Inline::of(InlineKind::math) },
        }
    }
}

/// Fully resolved character style.
#[napi(object)]
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

#[napi(string_enum)]
#[allow(non_camel_case_types)]
pub enum LinkTargetKind {
    /// Absolute URL with a scheme.
    external,
    /// Scheme-less relative reference, preserved as written.
    relative,
    /// Internal target: a heading anchor or an `anchor` inline.
    anchor,
}

#[napi(object)]
pub struct LinkTarget {
    pub kind: LinkTargetKind,
    /// The URL, relative reference, or anchor id.
    pub value: String,
}

impl From<model::LinkTarget> for LinkTarget {
    fn from(target: model::LinkTarget) -> Self {
        let (kind, value) = match target {
            model::LinkTarget::External(v) => (LinkTargetKind::external, v),
            model::LinkTarget::Relative(v) => (LinkTargetKind::relative, v),
            model::LinkTarget::Anchor(v) => (LinkTargetKind::anchor, v),
        };
        LinkTarget { kind, value }
    }
}

#[napi(string_enum)]
#[allow(non_camel_case_types)]
pub enum ImageSourceKind {
    /// Absolute URL with a scheme.
    external,
    /// Embedded image, carried in `Document.assets`.
    asset,
    /// No usable source: the image's part is missing or unreadable and it has
    /// no URL. Only the alt text remains.
    unavailable,
}

#[napi(object)]
pub struct ImageSource {
    pub kind: ImageSourceKind,
    /// external.
    pub url: Option<String>,
    /// asset: index into `Document.assets`.
    pub asset_id: Option<u32>,
}

impl From<model::ImageSource> for ImageSource {
    fn from(source: model::ImageSource) -> Self {
        match source {
            model::ImageSource::External(url) => {
                ImageSource { kind: ImageSourceKind::external, url: Some(url), asset_id: None }
            }
            model::ImageSource::Asset(id) => {
                ImageSource { kind: ImageSourceKind::asset, url: None, asset_id: Some(id.0 as u32) }
            }
            model::ImageSource::Unavailable => {
                ImageSource { kind: ImageSourceKind::unavailable, url: None, asset_id: None }
            }
        }
    }
}

/// The marker family a list uses in the source document.
#[napi(string_enum)]
#[allow(non_camel_case_types)]
pub enum MarkerKind {
    bullet,
    decimal,
    lowerAlpha,
    upperAlpha,
    lowerRoman,
    upperRoman,
}

impl From<model::MarkerKind> for MarkerKind {
    fn from(marker: model::MarkerKind) -> Self {
        match marker {
            model::MarkerKind::Bullet => MarkerKind::bullet,
            model::MarkerKind::Decimal => MarkerKind::decimal,
            model::MarkerKind::LowerAlpha => MarkerKind::lowerAlpha,
            model::MarkerKind::UpperAlpha => MarkerKind::upperAlpha,
            model::MarkerKind::LowerRoman => MarkerKind::lowerRoman,
            model::MarkerKind::UpperRoman => MarkerKind::upperRoman,
        }
    }
}

#[napi(object)]
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

#[napi(object)]
pub struct ListItem {
    pub blocks: Vec<Block>,
    /// Task-list state, when the item carries a checkbox.
    pub checked: Option<bool>,
    /// Literal marker text that overrides the list marker when the source
    /// number text cannot be reproduced from the marker and position alone
    /// (composite number text such as `1-a)`).
    pub marker_label: Option<String>,
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

#[napi(string_enum)]
#[allow(non_camel_case_types)]
pub enum TableKind {
    /// A real data table.
    data,
    /// Layout scaffolding (text boxes, positioning tables).
    layout,
}

/// Canonical table grid: every logical grid position appears exactly once.
/// Content and spans live on the origin slot, and each position a span covers
/// holds a `covered` slot pointing back at that origin.
#[napi(object)]
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
                model::TableKind::Data => TableKind::data,
                model::TableKind::Layout => TableKind::layout,
            },
        }
    }
}

#[napi(string_enum)]
#[allow(non_camel_case_types)]
pub enum CellSlotKind {
    origin,
    covered,
}

#[napi(object)]
pub struct CellSlot {
    pub kind: CellSlotKind,
    /// origin.
    pub cell: Option<Cell>,
    /// covered: row of the origin this position belongs to.
    pub origin_row: Option<u32>,
    /// covered: column of the origin this position belongs to.
    pub origin_col: Option<u32>,
}

impl From<model::CellSlot> for CellSlot {
    fn from(slot: model::CellSlot) -> Self {
        match slot {
            model::CellSlot::Origin(cell) => CellSlot {
                kind: CellSlotKind::origin,
                cell: Some(cell.into()),
                origin_row: None,
                origin_col: None,
            },
            model::CellSlot::Covered { origin_row, origin_col } => CellSlot {
                kind: CellSlotKind::covered,
                cell: None,
                origin_row: Some(origin_row as u32),
                origin_col: Some(origin_col as u32),
            },
        }
    }
}

#[napi(object)]
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

#[napi(string_enum)]
#[allow(non_camel_case_types)]
pub enum NoteKind {
    footnote,
    endnote,
}

#[napi(object)]
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
                model::NoteKind::Footnote => NoteKind::footnote,
                model::NoteKind::Endnote => NoteKind::endnote,
            },
            blocks: blocks(note.blocks),
        }
    }
}

/// An embedded binary asset (image, object payload). Bytes are always
/// retained, so a document stays self-contained.
#[napi(object)]
pub struct Asset {
    /// Index into `Document.assets`, as referenced by an image source.
    pub id: u32,
    /// MIME type, e.g. `image/png`.
    pub media_type: String,
    /// Package part or stream the asset came from, for provenance.
    pub origin_part: String,
    pub data: Buffer,
}

impl From<model::Asset> for Asset {
    fn from(asset: model::Asset) -> Self {
        Asset {
            id: asset.id.0 as u32,
            media_type: asset.media_type,
            origin_part: asset.origin_part,
            data: asset.bytes.into(),
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
