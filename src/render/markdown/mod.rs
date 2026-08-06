//! GitHub-Flavored Markdown serializer for the document model.

mod anchors;
mod escape;
mod inline;
mod table;

#[cfg(test)]
mod tests;

use crate::model::{Block, Document, Inline, List, MarkerKind, Note, TableKind, inlines_are_empty};
use anchors::{AnchorMap, resolve_anchors};
use escape::{EscapeOpts, InlineContext, backtick_fence, escape_text};
use inline::render_inlines;
use std::collections::{HashMap, HashSet};

/// Escape a source-derived composite marker label for literal use: control
/// characters collapse to spaces and Markdown syntax is neutralized so a
/// crafted label cannot alter document structure.
pub(crate) fn escape_marker_label(label: &str, ctx: InlineContext) -> String {
    let cleaned: String = label.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let opts = EscapeOpts {
        // List-item content re-opens block syntax after the `- ` marker.
        at_line_start: ctx == InlineContext::Block,
        trailing_active: true,
        ..Default::default()
    };
    escape_text(&cleaned, ctx, opts)
}

/// Footnote id -> rendered number, shared by all render functions.
type NoteNumbers = HashMap<String, usize>;

/// Immutable render context threaded through every render function.
pub(crate) struct Ctx {
    nums: NoteNumbers,
    anchors: AnchorMap,
}

pub fn document_to_markdown(doc: &Document) -> String {
    let rc = Ctx { nums: number_notes(doc), anchors: resolve_anchors(doc) };
    let mut parts: Vec<String> = doc.blocks.iter().filter_map(|b| render_block(b, &rc)).collect();
    let mut rendered_defs: HashSet<usize> = HashSet::new();
    let mut ordered: Vec<(&Note, usize)> =
        doc.notes.iter().filter_map(|n| rc.nums.get(&n.id).map(|&num| (n, num))).collect();
    ordered.sort_by_key(|(_, num)| *num);
    for (note, num) in ordered {
        // Duplicate note ids collapse to one number; render one definition.
        if !rendered_defs.insert(num) {
            log::debug!("duplicate note id {:?} dropped from output", note.id);
            continue;
        }
        let body = render_blocks(&note.blocks, &rc);
        if body.is_empty() {
            continue;
        }
        let mut lines = body.lines();
        let first = lines.next().unwrap_or("");
        let mut s = format!("[^{num}]: {first}");
        for line in lines {
            s.push('\n');
            if !line.is_empty() {
                s.push_str("    ");
                s.push_str(line);
            }
        }
        parts.push(s);
    }
    let mut out = parts.join("\n\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Number notes in first-reference order; unreferenced notes follow at the
/// end. The first note wins a duplicated id.
fn number_notes(doc: &Document) -> NoteNumbers {
    let mut valid: HashMap<&str, &Note> = HashMap::new();
    for note in &doc.notes {
        if !note.blocks.iter().all(block_is_blank) {
            valid.entry(note.id.as_str()).or_insert(note);
        }
    }
    let mut order: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    collect_note_refs(&doc.blocks, &valid, &mut order, &mut seen);
    for note in &doc.notes {
        if valid.contains_key(note.id.as_str()) && seen.insert(note.id.clone()) {
            order.push(note.id.clone());
        }
    }
    order.into_iter().enumerate().map(|(i, id)| (id, i + 1)).collect()
}

fn block_is_blank(block: &Block) -> bool {
    match block {
        Block::Paragraph(inlines) => inlines_are_empty(inlines),
        _ => false,
    }
}

fn collect_note_refs(
    blocks: &[Block],
    valid: &HashMap<&str, &Note>,
    order: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    fn walk_inlines(
        inlines: &[Inline],
        valid: &HashMap<&str, &Note>,
        order: &mut Vec<String>,
        seen: &mut HashSet<String>,
    ) {
        for inline in inlines {
            match inline {
                Inline::NoteRef(id) => {
                    if let Some(note) = valid.get(id.as_str())
                        && seen.insert(id.clone())
                    {
                        order.push(id.clone());
                        collect_note_refs(&note.blocks, valid, order, seen);
                    }
                }
                Inline::Link { content, .. } => walk_inlines(content, valid, order, seen),
                _ => {}
            }
        }
    }
    for block in blocks {
        match block {
            Block::Paragraph(i) | Block::Heading { content: i, .. } => {
                walk_inlines(i, valid, order, seen)
            }
            Block::List(list) => {
                for item in &list.items {
                    collect_note_refs(&item.blocks, valid, order, seen);
                }
            }
            Block::Table(t) => {
                for row in &t.grid {
                    for slot in row {
                        if let crate::model::CellSlot::Origin(cell) = slot {
                            collect_note_refs(&cell.blocks, valid, order, seen);
                        }
                    }
                }
            }
            Block::BlockQuote(blocks) => collect_note_refs(blocks, valid, order, seen),
            Block::CodeBlock { .. } | Block::Rule => {}
        }
    }
}

fn render_blocks(blocks: &[Block], rc: &Ctx) -> String {
    let parts: Vec<String> = blocks.iter().filter_map(|b| render_block(b, rc)).collect();
    parts.join("\n\n")
}

/// Whether a single-cell table's lone cell holds a nested table.
fn wraps_table(t: &crate::model::Table) -> bool {
    let crate::model::CellSlot::Origin(cell) = &t.grid[0][0] else { return false };
    cell.blocks.iter().any(|b| matches!(b, Block::Table(_)))
}

fn render_block(block: &Block, rc: &Ctx) -> Option<String> {
    match block {
        Block::Heading { level, content, .. } => {
            let text = render_inlines(content, InlineContext::Heading, rc);
            let text = text.trim();
            if text.is_empty() {
                return None;
            }
            let level = (*level).clamp(1, 6) as usize;
            Some(format!("{} {}", "#".repeat(level), text))
        }
        Block::Paragraph(inlines) => {
            let text = render_inlines(inlines, InlineContext::Block, rc);
            let trimmed = trim_paragraph(&text);
            if trimmed.is_empty() { None } else { Some(trimmed) }
        }
        Block::List(list) => render_list(list, rc),
        // Trivial layout tables are scaffolding; render their content directly.
        // A single-cell table wrapping a nested table is the same scaffolding
        // shape (Word forms often wrap their whole body in one): flattening
        // would collapse the inner grid into `a / b` runs, so unwrap the
        // wrapper and let the nested table render as a real one.
        Block::Table(t)
            if t.is_single_cell() && (t.kind == TableKind::Layout || wraps_table(t)) =>
        {
            let crate::model::CellSlot::Origin(cell) = &t.grid[0][0] else { unreachable!() };
            let inner = render_blocks(&cell.blocks, rc);
            if inner.is_empty() { None } else { Some(inner) }
        }
        Block::Table(t) => table::render_table(t, rc),
        Block::BlockQuote(blocks) => {
            let inner = render_blocks(blocks, rc);
            if inner.is_empty() {
                return None;
            }
            let quoted: Vec<String> = inner
                .lines()
                .map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") })
                .collect();
            Some(quoted.join("\n"))
        }
        Block::CodeBlock { lang, text } => {
            let fence = backtick_fence(text, 3);
            let lang = lang.as_deref().unwrap_or("");
            let body = text.trim_end_matches('\n');
            Some(format!("{fence}{lang}\n{body}\n{fence}"))
        }
        Block::Rule => Some("---".to_string()),
    }
}

fn render_list(list: &List, rc: &Ctx) -> Option<String> {
    if list.items.is_empty() {
        return None;
    }
    let mut rendered_items: Vec<String> = Vec::new();
    let mut loose = false;
    for (i, item) in list.items.iter().enumerate() {
        // GFM has decimal ordered lists only, so Roman/alphabetic levels
        // render as bullets carrying the source marker as literal text
        // (`- iv. …`) — the source marker semantics stay visible. Items with
        // an explicit label (composite number text) render it the same way.
        let marker = match (&item.marker_label, list.marker) {
            (Some(label), _) => {
                format!("- {} ", escape_marker_label(label, InlineContext::Block))
            }
            (None, MarkerKind::Bullet) => "- ".to_string(),
            (None, MarkerKind::Decimal) => format!("{}. ", list.start.saturating_add(i as u64)),
            (None, kind) => format!("- {} ", kind.label(list.start.saturating_add(i as u64))),
        };
        let checkbox = match item.checked {
            Some(true) => "[x] ",
            Some(false) => "[ ] ",
            None => "",
        };
        let body = render_blocks(&item.blocks, rc);
        if item.blocks.len() > 1 {
            loose = true;
        }
        let indent = " ".repeat(marker.chars().count());
        let mut lines = body.lines();
        let first = lines.next().unwrap_or("");
        let mut s = format!("{marker}{checkbox}{first}");
        for line in lines {
            s.push('\n');
            if line.is_empty() {
                loose = true;
            } else {
                s.push_str(&indent);
                s.push_str(line);
            }
        }
        rendered_items.push(s);
    }
    let sep = if loose { "\n\n" } else { "\n" };
    Some(rendered_items.join(sep))
}

/// Trim paragraph lines, keeping hard-break backslashes intact.
fn trim_paragraph(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| {
            let t = l.trim_start();
            let t = if ends_with_hard_break(t) { t } else { t.trim_end() };
            if t.trim_end_matches('\\').trim().is_empty() { "" } else { t }
        })
        .collect();
    let start = lines.iter().position(|l| !l.is_empty());
    let end = lines.iter().rposition(|l| !l.is_empty());
    match (start, end) {
        (Some(s), Some(e)) => {
            let mut out = lines[s..=e].join("\n");
            if ends_with_hard_break(&out) {
                out.pop();
                out.truncate(out.trim_end().len());
            }
            out
        }
        _ => String::new(),
    }
}

fn ends_with_hard_break(line: &str) -> bool {
    line.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1
}
