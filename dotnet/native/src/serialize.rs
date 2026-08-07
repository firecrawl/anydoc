//! Serialize the document model to plain JSON, shaped to match the Node and
//! Python bindings (string `kind` discriminants, optional variant payloads,
//! snake_case). Bytes are base64, suited to a byte-transfer ABI.

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{Map, Value};

use anydoc::model::{Asset, Block, Cell, CellSlot, Document, ImageSource, Inline, LinkTarget, List, ListItem, MarkerKind, Note, NoteKind, Style, Table};

/// A `serde_json` object builder with an ergonomic "set if present" helper.
struct Obj(Map<String, Value>);

impl Obj {
    fn new() -> Self {
        Self(Map::new())
    }

    fn set(mut self, key: &str, value: Value) -> Self {
        self.0.insert(key.to_string(), value);
        self
    }

    fn maybe(mut self, key: &str, value: Option<Value>) -> Self {
        if let Some(value) = value {
            self.0.insert(key.to_string(), value);
        }
        self
    }

    fn join(self) -> Value {
        Value::Object(self.0)
    }
}

pub fn str(value: impl Into<String>) -> Value {
    Value::String(value.into())
}

pub fn list(items: impl IntoIterator<Item = Value>) -> Value {
    Value::Array(items.into_iter().collect())
}

pub fn document(document: Document) -> Value {
    Obj::new()
        .set("blocks", list(document.blocks.into_iter().map(block)))
        .set("notes", list(document.notes.into_iter().map(note)))
        .set("assets", list(document.assets.into_iter().map(asset)))
        .join()
}

pub fn block(value: Block) -> Value {
    match value {
        Block::Heading { level, anchor, content } => Obj::new()
            .set("kind", str("heading"))
            .set("level", Value::from(level))
            .maybe("anchor", anchor.map(str))
            .set("content", list(content.into_iter().map(inline)))
            .join(),
        Block::Paragraph(content) => Obj::new()
            .set("kind", str("paragraph"))
            .set("content", list(content.into_iter().map(inline)))
            .join(),
        Block::List(v) => {
            Obj::new().set("kind", str("list")).set("list", list_value(&v)).join()
        }
        Block::Table(v) => {
            Obj::new().set("kind", str("table")).set("table", table(v)).join()
        }
        Block::BlockQuote(blocks) => Obj::new()
            .set("kind", str("block_quote"))
            .set("blocks", list(blocks.into_iter().map(block)))
            .join(),
        Block::CodeBlock { lang, text } => Obj::new()
            .set("kind", str("code_block"))
            .maybe("lang", lang.map(str))
            .set("text", str(text))
            .join(),
        Block::Rule => Obj::new().set("kind", str("rule")).join(),
    }
}

pub fn inline(value: Inline) -> Value {
    match value {
        Inline::Text { text, style: st } => Obj::new()
            .set("kind", str("text"))
            .set("text", str(text))
            .set("style", style(st))
            .join(),
        Inline::Link { content, target } => Obj::new()
            .set("kind", str("link"))
            .set("content", list(content.into_iter().map(inline)))
            .set("target", link_target(target))
            .join(),
        Inline::Image { alt, source } => Obj::new()
            .set("kind", str("image"))
            .set("alt", str(alt))
            .set("source", image_source(source))
            .join(),
        Inline::Anchor(id) => {
            Obj::new().set("kind", str("anchor")).set("anchor", str(id)).join()
        }
        Inline::NoteRef(id) => {
            Obj::new().set("kind", str("note_ref")).set("note_id", str(id)).join()
        }
        Inline::LineBreak => Obj::new().set("kind", str("line_break")).join(),
    }
}

pub fn style(style: Style) -> Value {
    Obj::new()
        .set("bold", Value::from(style.bold))
        .set("italic", Value::from(style.italic))
        .set("strike", Value::from(style.strike))
        .set("code", Value::from(style.code))
        .join()
}

pub fn link_target(target: LinkTarget) -> Value {
    match target {
        LinkTarget::External(value) => {
            Obj::new().set("kind", str("external")).set("value", str(value)).join()
        }
        LinkTarget::Relative(value) => {
            Obj::new().set("kind", str("relative")).set("value", str(value)).join()
        }
        LinkTarget::Anchor(value) => {
            Obj::new().set("kind", str("anchor")).set("value", str(value)).join()
        }
    }
}

pub fn image_source(source: ImageSource) -> Value {
    match source {
        ImageSource::External(url) => {
            Obj::new().set("kind", str("external")).set("url", str(url)).join()
        }
        ImageSource::Asset(id) => {
            Obj::new().set("kind", str("asset")).set("asset_id", Value::from(id.0)).join()
        }
        ImageSource::Unavailable => Obj::new().set("kind", str("unavailable")).join(),
    }
}

pub fn marker(marker: MarkerKind) -> Value {
    str(match marker {
        MarkerKind::Bullet => "bullet",
        MarkerKind::Decimal => "decimal",
        MarkerKind::LowerAlpha => "lower_alpha",
        MarkerKind::UpperAlpha => "upper_alpha",
        MarkerKind::LowerRoman => "lower_roman",
        MarkerKind::UpperRoman => "upper_roman",
    })
}

pub fn list_value(value: &List) -> Value {
    Obj::new()
        .set("marker", marker(value.marker))
        .set("start", Value::from(value.start))
        .set("items", list(value.items.iter().map(list_item)))
        .join()
}

pub fn list_item(item: &ListItem) -> Value {
    Obj::new()
        .set("blocks", list(item.blocks.iter().cloned().map(block)))
        .maybe("checked", item.checked.map(Value::from))
        .maybe("marker_label", item.marker_label.clone().map(str))
        .join()
}

pub fn table(table: Table) -> Value {
    Obj::new()
        .set(
            "grid",
            list(
                table
                    .grid
                    .iter()
                    .map(|row| list(row.iter().map(cell_slot))),
            ),
        )
        .set("header_rows", Value::from(table.header_rows))
        .set(
            "kind",
            str(match table.kind {
                anydoc::model::TableKind::Data => "data",
                anydoc::model::TableKind::Layout => "layout",
            }),
        )
        .join()
}

pub fn cell_slot(slot: &CellSlot) -> Value {
    match slot {
        CellSlot::Origin(v) => {
            Obj::new().set("kind", str("origin")).set("cell", cell(v)).join()
        }
        CellSlot::Covered { origin_row, origin_col } => Obj::new()
            .set("kind", str("covered"))
            .set("origin_row", Value::from(*origin_row))
            .set("origin_col", Value::from(*origin_col))
            .join(),
    }
}

pub fn cell(cell: &Cell) -> Value {
    Obj::new()
        .set("blocks", list(cell.blocks.iter().cloned().map(block)))
        .set("col_span", Value::from(cell.col_span))
        .set("row_span", Value::from(cell.row_span))
        .join()
}

pub fn note(note: Note) -> Value {
    Obj::new()
        .set("id", str(note.id))
        .set(
            "kind",
            str(match note.kind {
                NoteKind::Footnote => "footnote",
                NoteKind::Endnote => "endnote",
            }),
        )
        .set("blocks", list(note.blocks.into_iter().map(block)))
        .join()
}

pub fn asset(asset: Asset) -> Value {
    Obj::new()
        .set("id", Value::from(asset.id.0))
        .set("media_type", str(asset.media_type))
        .set("origin_part", str(asset.origin_part))
        .set("data", str(STANDARD.encode(&asset.bytes)))
        .join()
}