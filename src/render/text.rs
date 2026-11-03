//! Plain-text serializer for the document model. Fork (sznb): `to_text_bytes`
//! produces readable plain text directly — headings and paragraphs as text
//! lines, tables as tab-joined rows (merged content appears once, on its
//! origin slot; covered positions emit nothing), nested tables appended
//! after their host.

use crate::model::{Block, Cell, CellSlot, Document, List, MarkerKind};
use crate::render::markdown::NoteNumbers;
use crate::render::markdown::inline::inlines_to_plain;
use crate::render::markdown::number_notes;

pub fn document_to_text(doc: &Document) -> String {
    let nums = number_notes(doc);
    let mut parts: Vec<String> = Vec::new();
    for block in &doc.blocks {
        if let Some(s) = render_block(block, &nums) {
            parts.push(s);
        }
    }
    if !doc.notes.is_empty() {
        let mut section = String::from("── 脚注 ──");
        for note in &doc.notes {
            if let Some(n) = nums.get(&note.id) {
                let body = render_blocks(&note.blocks, &nums).replace('\n', " ");
                let body = body.trim();
                if !body.is_empty() {
                    section.push_str(&format!("\n[{n}] {body}"));
                }
            }
        }
        if section.len() > "── 脚注 ──".len() {
            parts.push(section);
        }
    }
    let mut out = parts.join("\n\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

fn render_blocks(blocks: &[Block], nums: &NoteNumbers) -> String {
    let parts: Vec<String> = blocks.iter().filter_map(|b| render_block(b, nums)).collect();
    parts.join("\n\n")
}

fn render_block(block: &Block, nums: &NoteNumbers) -> Option<String> {
    match block {
        Block::Heading { content, .. } | Block::Paragraph(content) => {
            let mut s = String::new();
            inlines_to_plain(content, nums, &mut s);
            // Fork (sznb): keep in-paragraph line breaks as bare newlines
            // (matching the original pipeline); only edges trim.
            let s = s.trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        }
        Block::List(list) => render_list(list, nums),
        Block::Table(t) => render_table_text(t, nums),
        Block::BlockQuote(blocks) => {
            let inner = render_blocks(blocks, nums);
            if inner.is_empty() { None } else { Some(inner) }
        }
        Block::CodeBlock { text, .. } | Block::Math(text) => {
            let t = text.trim();
            if t.is_empty() { None } else { Some(t.to_string()) }
        }
        Block::Rule => None,
    }
}

fn render_list(list: &List, nums: &NoteNumbers) -> Option<String> {
    if list.items.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = Vec::new();
    for (i, item) in list.items.iter().enumerate() {
        let body = render_blocks(&item.blocks, nums).trim().to_string();
        if body.is_empty() {
            continue;
        }
        let marker = match (&item.marker_label, list.marker) {
            (Some(label), _) => label.clone(),
            (None, MarkerKind::Bullet) => "-".to_string(),
            (None, kind) => kind.label(list.start.saturating_add(i as u64)),
        };
        lines.push(format!("{marker} {body}"));
    }
    if lines.is_empty() { None } else { Some(lines.join("\n")) }
}

fn render_table_text(table: &crate::model::Table, nums: &NoteNumbers) -> Option<String> {
    if table.grid.is_empty() {
        return None;
    }
    let mut rows: Vec<String> = Vec::new();
    let mut nested: Vec<String> = Vec::new();
    for row in &table.grid {
        // Fork (sznb): every grid position emits (origin -> text, covered ->
        // empty). Plain text has no span syntax, so the per-column tab
        // alignment is the only column structure that survives; skipping
        // blank/covered slots collapses neighbouring columns together.
        let mut cells: Vec<String> = Vec::new();
        let mut has_content = false;
        for slot in row {
            match slot {
                CellSlot::Origin(cell) => {
                    let parts = cell_plain_parts(cell, nums, &mut nested);
                    if parts.is_empty() {
                        cells.push(String::new());
                    } else {
                        has_content = true;
                        cells.push(parts.join(" / "));
                    }
                }
                CellSlot::Covered { .. } => cells.push(String::new()),
            }
        }
        if has_content {
            rows.push(cells.join("\t"));
        }
    }
    for tbl in nested {
        rows.push(String::new());
        rows.push(tbl);
    }
    if rows.is_empty() { None } else { Some(rows.join("\n")) }
}

/// Cell content flattened to plain parts (one per paragraph-ish block);
/// nested tables render into `nested` for independent appending.
fn cell_plain_parts(cell: &Cell, nums: &NoteNumbers, nested: &mut Vec<String>) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    fn walk(blocks: &[Block], nums: &NoteNumbers, parts: &mut Vec<String>, nested: &mut Vec<String>) {
        for block in blocks {
            match block {
                Block::Heading { content, .. } | Block::Paragraph(content) => {
                    let mut s = String::new();
                    inlines_to_plain(content, nums, &mut s);
                    let s = s.replace('\n', " ").trim().to_string();
                    if !s.is_empty() {
                        parts.push(s);
                    }
                }
                Block::List(list) => {
                    // Fork (sznb): list items inside a table cell join with
                    // newlines (one item per line reads far better than a
                    // single run-on line in plain text).
                    let mut items: Vec<String> = Vec::new();
                    for (i, item) in list.items.iter().enumerate() {
                        let mut inner: Vec<String> = Vec::new();
                        walk(&item.blocks, nums, &mut inner, nested);
                        if inner.is_empty() {
                            continue;
                        }
                        let marker = match (&item.marker_label, list.marker) {
                            (Some(label), _) => label.clone(),
                            (None, MarkerKind::Bullet) => "-".to_string(),
                            (None, kind) => kind.label(list.start.saturating_add(i as u64)),
                        };
                        items.push(format!("{marker} {}", inner.join(" ")));
                    }
                    if !items.is_empty() {
                        parts.push(items.join("\n"));
                    }
                }
                Block::Table(t) => {
                    if let Some(rendered) = render_table_text(t, nums) {
                        nested.push(rendered);
                    }
                }
                Block::BlockQuote(inner) => walk(inner, nums, parts, nested),
                Block::CodeBlock { text, .. } | Block::Math(text) => {
                    let t = text.trim().replace('\n', " ");
                    if !t.is_empty() {
                        parts.push(t);
                    }
                }
                Block::Rule => {}
            }
        }
    }
    walk(&cell.blocks, nums, &mut parts, nested);
    parts
}
