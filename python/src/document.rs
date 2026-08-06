//! The document model, converted eagerly into Python objects.
//!
//! Every class is frozen: the model is a parse result, not a builder. Variant
//! enums (block kinds, link targets, ...) surface as snake_case strings on a
//! `kind` attribute, with the variant's payload on optional attributes, the
//! same shape the Node bindings give them.

use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyList};

use anydoc::model;

#[pyclass(frozen, get_all, module = "anydoc")]
pub struct Document {
    /// list[Block]
    blocks: Py<PyList>,
    /// list[OutlineEntry], in document reading order.
    outline: Py<PyList>,
    /// Footnote and endnote bodies, referenced from text by a `note_ref`
    /// inline. list[Note]
    notes: Py<PyList>,
    /// list[Asset]
    assets: Py<PyList>,
}

#[pyclass(frozen, get_all, module = "anydoc")]
pub struct OutlineEntry {
    /// Source outline depth, 1-based.
    level: u8,
    /// Heading text with styling and links flattened away.
    text: String,
    /// Stable source anchor, when the heading carries one.
    anchor: Option<String>,
}

fn outline_entry(entry: model::OutlineEntry) -> OutlineEntry {
    OutlineEntry { level: entry.level, text: entry.text, anchor: entry.anchor }
}

#[pyclass(frozen, get_all, module = "anydoc")]
pub struct Block {
    /// heading, paragraph, list, table, block_quote, code_block, or rule.
    kind: &'static str,
    /// heading: 1-6.
    level: Option<u8>,
    /// heading: stable anchor id when the document targets this heading.
    anchor: Option<String>,
    /// heading, paragraph: list[Inline].
    content: Option<Py<PyList>>,
    list: Option<Py<List>>,
    table: Option<Py<Table>>,
    /// block_quote: list[Block].
    blocks: Option<Py<PyList>>,
    /// code_block.
    lang: Option<String>,
    /// code_block.
    text: Option<String>,
}

impl Block {
    fn of(kind: &'static str) -> Block {
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

fn block(py: Python<'_>, block: model::Block) -> PyResult<Block> {
    Ok(match block {
        model::Block::Heading { level, anchor, content } => Block {
            level: Some(level),
            anchor,
            content: Some(inlines(py, content)?),
            ..Block::of("heading")
        },
        model::Block::Paragraph(content) => {
            Block { content: Some(inlines(py, content)?), ..Block::of("paragraph") }
        }
        model::Block::List(inner) => {
            Block { list: Some(Py::new(py, list(py, inner)?)?), ..Block::of("list") }
        }
        model::Block::Table(inner) => {
            Block { table: Some(Py::new(py, table(py, inner)?)?), ..Block::of("table") }
        }
        model::Block::BlockQuote(inner) => {
            Block { blocks: Some(blocks(py, inner)?), ..Block::of("block_quote") }
        }
        model::Block::CodeBlock { lang, text } => {
            Block { lang, text: Some(text), ..Block::of("code_block") }
        }
        model::Block::Rule => Block::of("rule"),
    })
}

#[pyclass(frozen, get_all, module = "anydoc")]
pub struct Inline {
    /// text, link, image, anchor (a zero-width marker for an internal link
    /// target at this position), note_ref, or line_break.
    kind: &'static str,
    /// text.
    text: Option<String>,
    /// text.
    style: Option<Py<Style>>,
    /// link: list[Inline].
    content: Option<Py<PyList>>,
    /// link.
    target: Option<Py<LinkTarget>>,
    /// image.
    alt: Option<String>,
    /// image.
    source: Option<Py<ImageSource>>,
    /// anchor: the anchor id.
    anchor: Option<String>,
    /// note_ref: the id of the note in `Document.notes`.
    note_id: Option<String>,
}

impl Inline {
    fn of(kind: &'static str) -> Inline {
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

fn inline(py: Python<'_>, inline: model::Inline) -> PyResult<Inline> {
    Ok(match inline {
        model::Inline::Text { text, style } => Inline {
            text: Some(text),
            style: Some(Py::new(py, Style::from(style))?),
            ..Inline::of("text")
        },
        model::Inline::Link { content, target } => Inline {
            content: Some(inlines(py, content)?),
            target: Some(Py::new(py, LinkTarget::from(target))?),
            ..Inline::of("link")
        },
        model::Inline::Image { alt, source } => Inline {
            alt: Some(alt),
            source: Some(Py::new(py, ImageSource::from(source))?),
            ..Inline::of("image")
        },
        model::Inline::Anchor(id) => Inline { anchor: Some(id), ..Inline::of("anchor") },
        model::Inline::NoteRef(id) => Inline { note_id: Some(id), ..Inline::of("note_ref") },
        model::Inline::LineBreak => Inline::of("line_break"),
    })
}

/// Fully resolved character style.
#[pyclass(frozen, get_all, module = "anydoc")]
pub struct Style {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
}

impl From<model::Style> for Style {
    fn from(style: model::Style) -> Self {
        Style { bold: style.bold, italic: style.italic, strike: style.strike, code: style.code }
    }
}

#[pyclass(frozen, get_all, module = "anydoc")]
pub struct LinkTarget {
    /// external (absolute URL with a scheme), relative (scheme-less relative
    /// reference, preserved as written), or anchor (internal target: a
    /// heading anchor or an `anchor` inline).
    kind: &'static str,
    /// The URL, relative reference, or anchor id.
    value: String,
}

impl From<model::LinkTarget> for LinkTarget {
    fn from(target: model::LinkTarget) -> Self {
        let (kind, value) = match target {
            model::LinkTarget::External(v) => ("external", v),
            model::LinkTarget::Relative(v) => ("relative", v),
            model::LinkTarget::Anchor(v) => ("anchor", v),
        };
        LinkTarget { kind, value }
    }
}

#[pyclass(frozen, get_all, module = "anydoc")]
pub struct ImageSource {
    /// external (absolute URL with a scheme), asset (embedded image, carried
    /// in `Document.assets`), or unavailable (no usable source: the image's
    /// part is missing or unreadable and it has no URL; only the alt text
    /// remains).
    kind: &'static str,
    /// external.
    url: Option<String>,
    /// asset: index into `Document.assets`.
    asset_id: Option<usize>,
}

impl From<model::ImageSource> for ImageSource {
    fn from(source: model::ImageSource) -> Self {
        match source {
            model::ImageSource::External(url) => {
                ImageSource { kind: "external", url: Some(url), asset_id: None }
            }
            model::ImageSource::Asset(id) => {
                ImageSource { kind: "asset", url: None, asset_id: Some(id.0) }
            }
            model::ImageSource::Unavailable => {
                ImageSource { kind: "unavailable", url: None, asset_id: None }
            }
        }
    }
}

#[pyclass(frozen, get_all, module = "anydoc")]
pub struct List {
    /// The marker family the list uses in the source document: bullet,
    /// decimal, lower_alpha, upper_alpha, lower_roman, or upper_roman.
    marker: &'static str,
    /// Ordinal the first item counts from.
    start: u64,
    /// list[ListItem]
    items: Py<PyList>,
}

fn list(py: Python<'_>, list: model::List) -> PyResult<List> {
    let marker = match list.marker {
        model::MarkerKind::Bullet => "bullet",
        model::MarkerKind::Decimal => "decimal",
        model::MarkerKind::LowerAlpha => "lower_alpha",
        model::MarkerKind::UpperAlpha => "upper_alpha",
        model::MarkerKind::LowerRoman => "lower_roman",
        model::MarkerKind::UpperRoman => "upper_roman",
    };
    let items = list.items.into_iter().map(|item| list_item(py, item));
    Ok(List { marker, start: list.start, items: pylist(py, items)? })
}

#[pyclass(frozen, get_all, module = "anydoc")]
pub struct ListItem {
    /// list[Block]
    blocks: Py<PyList>,
    /// Task-list state, when the item carries a checkbox.
    checked: Option<bool>,
    /// Literal marker text that overrides the list marker when the source
    /// number text cannot be reproduced from the marker and position alone
    /// (composite number text such as `1-a)`).
    marker_label: Option<String>,
}

fn list_item(py: Python<'_>, item: model::ListItem) -> PyResult<ListItem> {
    Ok(ListItem {
        blocks: blocks(py, item.blocks)?,
        checked: item.checked,
        marker_label: item.marker_label,
    })
}

/// Canonical table grid: every logical grid position appears exactly once.
/// Content and spans live on the origin slot, and each position a span covers
/// holds a `covered` slot pointing back at that origin.
#[pyclass(frozen, get_all, module = "anydoc")]
pub struct Table {
    /// list[list[CellSlot]]
    grid: Py<PyList>,
    /// Number of leading rows that are header rows (0 = no header).
    header_rows: usize,
    /// data (a real data table) or layout (layout scaffolding: text boxes,
    /// positioning tables).
    kind: &'static str,
}

fn table(py: Python<'_>, table: model::Table) -> PyResult<Table> {
    let rows = table
        .grid
        .into_iter()
        .map(|row| pylist(py, row.into_iter().map(|slot| cell_slot(py, slot))));
    Ok(Table {
        grid: pylist(py, rows)?,
        header_rows: table.header_rows,
        kind: match table.kind {
            model::TableKind::Data => "data",
            model::TableKind::Layout => "layout",
        },
    })
}

#[pyclass(frozen, get_all, module = "anydoc")]
pub struct CellSlot {
    /// origin or covered.
    kind: &'static str,
    /// origin.
    cell: Option<Py<Cell>>,
    /// covered: row of the origin this position belongs to.
    origin_row: Option<usize>,
    /// covered: column of the origin this position belongs to.
    origin_col: Option<usize>,
}

fn cell_slot(py: Python<'_>, slot: model::CellSlot) -> PyResult<CellSlot> {
    Ok(match slot {
        model::CellSlot::Origin(inner) => CellSlot {
            kind: "origin",
            cell: Some(Py::new(py, cell(py, inner)?)?),
            origin_row: None,
            origin_col: None,
        },
        model::CellSlot::Covered { origin_row, origin_col } => CellSlot {
            kind: "covered",
            cell: None,
            origin_row: Some(origin_row),
            origin_col: Some(origin_col),
        },
    })
}

#[pyclass(frozen, get_all, module = "anydoc")]
pub struct Cell {
    /// list[Block]
    blocks: Py<PyList>,
    col_span: u32,
    row_span: u32,
}

fn cell(py: Python<'_>, cell: model::Cell) -> PyResult<Cell> {
    Ok(Cell { blocks: blocks(py, cell.blocks)?, col_span: cell.col_span, row_span: cell.row_span })
}

#[pyclass(frozen, get_all, module = "anydoc")]
pub struct Note {
    id: String,
    /// footnote or endnote.
    kind: &'static str,
    /// list[Block]
    blocks: Py<PyList>,
}

fn note(py: Python<'_>, note: model::Note) -> PyResult<Note> {
    Ok(Note {
        id: note.id,
        kind: match note.kind {
            model::NoteKind::Footnote => "footnote",
            model::NoteKind::Endnote => "endnote",
        },
        blocks: blocks(py, note.blocks)?,
    })
}

/// An embedded binary asset (image, object payload). Bytes are always
/// retained, so a document stays self-contained.
#[pyclass(frozen, get_all, module = "anydoc")]
pub struct Asset {
    /// Index into `Document.assets`, as referenced by an image source.
    id: usize,
    /// MIME type, e.g. `image/png`.
    media_type: String,
    /// Package part or stream the asset came from, for provenance.
    origin_part: String,
    data: Py<PyBytes>,
}

fn asset(py: Python<'_>, asset: model::Asset) -> PyResult<Asset> {
    Ok(Asset {
        id: asset.id.0,
        media_type: asset.media_type,
        origin_part: asset.origin_part,
        data: PyBytes::new(py, &asset.bytes).unbind(),
    })
}

pub fn document(py: Python<'_>, document: model::Document) -> PyResult<Document> {
    let outline = document.outline();
    Ok(Document {
        blocks: blocks(py, document.blocks)?,
        outline: pylist(py, outline.into_iter().map(|entry| Ok(outline_entry(entry))))?,
        notes: pylist(py, document.notes.into_iter().map(|n| note(py, n)))?,
        assets: pylist(py, document.assets.into_iter().map(|a| asset(py, a)))?,
    })
}

fn blocks(py: Python<'_>, items: Vec<model::Block>) -> PyResult<Py<PyList>> {
    pylist(py, items.into_iter().map(|b| block(py, b)))
}

fn inlines(py: Python<'_>, items: Vec<model::Inline>) -> PyResult<Py<PyList>> {
    pylist(py, items.into_iter().map(|i| inline(py, i)))
}

fn pylist<'py, T: IntoPyObject<'py>>(
    py: Python<'py>,
    items: impl Iterator<Item = PyResult<T>>,
) -> PyResult<Py<PyList>> {
    let items = items.collect::<PyResult<Vec<T>>>()?;
    Ok(PyList::new(py, items)?.unbind())
}
