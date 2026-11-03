//! GitHub-Flavored Markdown serializer for the document model.

mod anchors;
mod escape;
pub(crate) mod inline;
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
pub(crate) type NoteNumbers = HashMap<String, usize>;

/// Immutable render context threaded through every render function.
pub(crate) struct Ctx {
    pub(crate) nums: NoteNumbers,
    anchors: AnchorMap,
    /// Fork (sznb): tables render as HTML `<table>` (true, default) or as
    /// GFM pipe tables (false) — the conversion service's `table_as_html`
    /// parameter reaches the fork through this flag.
    pub(crate) html_tables: bool,
}

/// Fork (sznb): render with the table form selected by `html_tables`
/// (`true` = HTML tables, the historical default).
pub fn document_to_markdown_opts(doc: &Document, html_tables: bool) -> String {
    let rc = Ctx { nums: number_notes(doc), anchors: resolve_anchors(doc), html_tables };
    let mut parts: Vec<(bool, String)> = doc
        .blocks
        .iter()
        .filter_map(|b| render_block(b, &rc).map(|s| (block_needs_blank(b), s)))
        .collect();
    let mut rendered_defs: HashSet<usize> = HashSet::new();
    let mut ordered: Vec<(&Note, usize)> =
        doc.notes.iter().filter_map(|n| rc.nums.get(&n.id).map(|&num| (n, num))).collect();
    ordered.sort_by_key(|(_, num)| *num);
    for (note, num) in ordered {
        let body = render_blocks(&note.blocks, &rc);
        if body.is_empty() {
            continue;
        }
        // The first non-empty definition for a duplicate id wins.
        if !rendered_defs.insert(num) {
            log::debug!("duplicate note id {:?} dropped from output", note.id);
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
        parts.push((true, s));
    }
    let out = join_block_parts(&parts);
    if out.is_empty() {
        return out;
    }
    // Fork (sznb): trailing blank paragraphs collapse, one final newline.
    let trimmed = out.trim_end_matches('\n');
    if trimmed.is_empty() {
        return String::new();
    }
    format!("{trimmed}\n")
}

/// Fork (sznb): block separation follows the original pipeline's line
/// layout — paragraphs, headings, lists and quotes join with a single
/// newline (no blank line), while tables, code blocks, math and rules keep
/// a blank line around them so the emitted structures still parse.
fn block_needs_blank(b: &Block) -> bool {
    matches!(b, Block::Table(_) | Block::CodeBlock { .. } | Block::Math(_) | Block::Rule)
}

fn join_block_parts(parts: &[(bool, String)]) -> String {
    let mut out = String::new();
    let mut prev_blank = true;
    for (blank, s) in parts {
        if s.is_empty() {
            // Fork (sznb): a blank paragraph renders as one blank line;
            // consecutive ones fold, leading ones drop.
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
                prev_blank = false;
            }
            continue;
        }
        if !out.is_empty() {
            // A blank paragraph just emitted a line break; the structural
            // blank of a table/code block is then already satisfied.
            if out.ends_with('\n') {
                out.push('\n');
            } else {
                out.push_str(if *blank || prev_blank { "\n\n" } else { "\n" });
            }
        }
        out.push_str(s);
        prev_blank = *blank;
    }
    out
}

/// Number notes in first-reference order; unreferenced notes follow at the
/// end. The first note wins a duplicated id.
pub(crate) fn number_notes(doc: &Document) -> NoteNumbers {
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
            Block::CodeBlock { .. } | Block::Rule | Block::Math(_) => {}
        }
    }
}

fn render_blocks(blocks: &[Block], rc: &Ctx) -> String {
    let parts: Vec<(bool, String)> = blocks
        .iter()
        .filter_map(|b| render_block(b, rc).map(|s| (block_needs_blank(b), s)))
        .collect();
    join_block_parts(&parts)
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
            // Fork (sznb): an empty paragraph renders as a blank line (the
            // original pipeline keeps the source's spacer paragraphs).
            if trimmed.is_empty() { Some(String::new()) } else { Some(trimmed) }
        }
        Block::List(list) => render_list(list, rc),
        // Trivial layout tables are scaffolding; render their content directly.
        Block::Table(t) if t.kind == TableKind::Layout && t.is_single_cell() => {
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
        Block::Math(tex) => {
            let tex = tex.trim();
            if tex.is_empty() {
                return None;
            }
            // A bare `$` is never valid inside math; escaped, it cannot
            // close the block early.
            let mut source = String::with_capacity(tex.len());
            let mut backslashes = 0;
            for c in tex.chars() {
                if c == '$' && backslashes % 2 == 0 {
                    source.push('\\');
                }
                source.push(c);
                backslashes = if c == '\\' { backslashes + 1 } else { 0 };
            }
            Some(format!("$$\n{source}\n$$"))
        }
    }
}

fn render_list(list: &List, rc: &Ctx) -> Option<String> {
    if list.items.is_empty() {
        return None;
    }
    let mut rendered_items: Vec<String> = Vec::new();
    let mut loose = false;
    for (i, item) in list.items.iter().enumerate() {
        // Fork (sznb): the source marker renders as-is, without stacking a
        // `- ` bullet in front. GFM ordered lists only parse `1.` decimals,
        // so `1、` / `iv.` / composite labels stay literal text lines that
        // mirror the source — the Chinese-official-document corpus reads
        // best without the doubled prefix (`- 1、 …`).
        let marker = match (&item.marker_label, list.marker) {
            (Some(label), _) => {
                format!("{} ", escape_marker_label(label, InlineContext::Block))
            }
            (None, MarkerKind::Bullet) => "- ".to_string(),
            (None, MarkerKind::Decimal) => format!("{}. ", list.start.saturating_add(i as u64)),
            (None, kind) => format!("{} ", kind.label(list.start.saturating_add(i as u64))),
        };
        let body = render_blocks(&item.blocks, rc);
        if item.blocks.len() > 1 {
            loose = true;
        }
        let indent = " ".repeat(marker.chars().count());
        let mut lines = body.lines();
        let first = lines.next().unwrap_or("");
        let mut s = format!("{marker}{first}");
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

/// Trim paragraph lines. Fork (sznb): in-paragraph line breaks render as
/// bare newlines (no `\` hard-break marker), so every line simply trims its
/// edges and the paragraph keeps its interior newlines.
fn trim_paragraph(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| {
            let t = l.trim();
            if t.is_empty() { "" } else { t }
        })
        .collect();
    let start = lines.iter().position(|l| !l.is_empty());
    let end = lines.iter().rposition(|l| !l.is_empty());
    match (start, end) {
        (Some(s), Some(e)) => lines[s..=e].join("\n"),
        _ => String::new(),
    }
}
