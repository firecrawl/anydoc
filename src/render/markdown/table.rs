//! Table rendering over the canonical grid. Fork (sznb): two forms, selected
//! by the render context's `html_tables` flag (the conversion service's
//! `table_as_html` parameter):
//!
//! - **HTML** (default): `<table>` markup carrying the span structure on the
//!   origin slots — `colspan`/`rowspan` preserve merged cells exactly as the
//!   source had them; covered positions emit nothing. Cell text is
//!   HTML-escaped plain text; nested tables render as independent tables
//!   appended after the host table, recursively.
//! - **GFM**: pipe tables (the upstream renderer, kept verbatim apart from
//!   the fork-wide "no `**`" policy). Covered positions render as blank
//!   cells — GFM has no span syntax; the grid invariant means the renderer
//!   never synthesizes or duplicates columns.

use crate::model::{Block, Cell, CellSlot, Table};
use crate::render::markdown::Ctx;
use crate::render::markdown::escape::InlineContext;
use crate::render::markdown::inline::{
    push_code_span, push_math_span, render_inlines, inlines_to_plain,
};

pub(crate) fn render_table(table: &Table, rc: &Ctx) -> Option<String> {
    if rc.html_tables {
        render_table_html(table, rc)
    } else {
        render_table_gfm(table, rc)
    }
}

// ---------- HTML form ----------

/// HTML-escape a text run for cell content. Order matters: `&` first.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// True when no block in the cell yields any non-whitespace text. A cell
/// holding only a nested table is NOT empty: the nested table renders after
/// the host, and treating it as blank once dropped a whole 1x1 host table
/// together with everything nested inside it (content loss on real
/// government documents that wrap report tables in a layout host).
fn cell_is_empty(cell: &Cell, rc: &Ctx) -> bool {
    if cell.blocks.iter().any(|b| matches!(b, Block::Table(_))) {
        return false;
    }
    let mut probe = Vec::new();
    cell_block_plain(&cell.blocks, rc, &mut probe, &mut Vec::new());
    probe.iter().all(|p| p.trim().is_empty())
}

fn render_table_html(table: &Table, rc: &Ctx) -> Option<String> {
    if table.grid.is_empty() {
        return None;
    }
    // Trailing rows whose every slot is blank (covered, or an empty origin)
    // are scaffolding; drop them, as the GFM renderer did.
    let last_meaningful = table
        .grid
        .iter()
        .rposition(|row| {
            row.iter().any(|slot| match slot {
                CellSlot::Origin(cell) => !cell_is_empty(cell, rc),
                CellSlot::Covered { .. } => false,
            })
        })?;
    let mut lines: Vec<String> = vec!["<table>".to_string()];
    let mut nested: Vec<String> = Vec::new();
    for (ri, row) in table.grid.iter().take(last_meaningful + 1).enumerate() {
        lines.push("  <tr>".to_string());
        let tag = if ri < table.header_rows { "th" } else { "td" };
        for slot in row {
            // Covered positions belong to an origin elsewhere; emitting them
            // would duplicate merged content.
            let CellSlot::Origin(cell) = slot else { continue };
            let mut attrs = String::new();
            if cell.col_span > 1 {
                attrs.push_str(&format!(" colspan=\"{}\"", cell.col_span));
            }
            if cell.row_span > 1 {
                attrs.push_str(&format!(" rowspan=\"{}\"", cell.row_span));
            }
            let text = render_cell_html(cell, rc, &mut nested);
            lines.push(format!("    <{tag}{attrs}>{text}</{tag}>"));
        }
        lines.push("  </tr>".to_string());
    }
    lines.push("</table>".to_string());
    for tbl in nested {
        lines.push(String::new());
        lines.push(tbl);
    }
    Some(lines.join("\n"))
}

/// Cell content as escaped HTML: paragraph-level blocks joined with `<br>`,
/// and in-paragraph line breaks kept as `<br>` too (the source's own
/// line-break positions — stacked unit names, split titles — are layout
/// information worth preserving); nested tables are collected for
/// independent rendering after the host table.
fn render_cell_html(cell: &Cell, rc: &Ctx, nested: &mut Vec<String>) -> String {
    let mut parts: Vec<String> = Vec::new();
    cell_block_plain(&cell.blocks, rc, &mut parts, nested);
    parts
        .iter()
        .map(|p| html_escape(p).replace('\n', "<br>"))
        .collect::<Vec<_>>()
        .join("<br>")
}

/// Flatten cell blocks into plain-text parts (one per paragraph-ish block).
fn cell_block_plain(blocks: &[Block], rc: &Ctx, parts: &mut Vec<String>, nested: &mut Vec<String>) {
    for block in blocks {
        match block {
            Block::Heading { content, .. } | Block::Paragraph(content) => {
                let mut s = String::new();
                inlines_to_plain(content, &rc.nums, &mut s);
                // Keep interior line breaks; render_cell_html turns them
                // into `<br>`. Only edge whitespace is padding.
                if !s.trim().is_empty() {
                    parts.push(s.trim().to_string());
                }
            }
            Block::List(list) => {
                for (i, item) in list.items.iter().enumerate() {
                    let mut inner: Vec<String> = Vec::new();
                    cell_block_plain(&item.blocks, rc, &mut inner, nested);
                    if inner.is_empty() {
                        continue;
                    }
                    let marker = match (&item.marker_label, list.marker) {
                        (Some(label), _) => label.clone(),
                        (None, crate::model::MarkerKind::Bullet) => "-".to_string(),
                        (None, kind) => kind.label(list.start.saturating_add(i as u64)),
                    };
                    parts.push(format!("{marker} {}", inner.join(" ")));
                }
            }
            Block::Table(t) => {
                // Nested table: rendered after the host table, host cell
                // stays free of its content (mirrors the V2 convention).
                if let Some(rendered) = render_table(t, rc) {
                    nested.push(rendered);
                }
            }
            Block::BlockQuote(inner) => cell_block_plain(inner, rc, parts, nested),
            Block::CodeBlock { text, .. } | Block::Math(text) => {
                let t = text.trim();
                if !t.is_empty() {
                    parts.push(t.replace('\n', " "));
                }
            }
            Block::Rule => {}
        }
    }
}

// ---------- GFM form (upstream renderer, fork: no `**` in cells) ----------

struct RenderedCell {
    text: String,
    covered_span: bool,
}

fn render_table_gfm(table: &Table, rc: &Ctx) -> Option<String> {
    // Interior empty rows render as blank rows — they carry the source's
    // row coordinates; trailing blank rows are popped below.
    if table.grid.is_empty() {
        return None;
    }
    let width = table.grid.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut rendered: Vec<Vec<RenderedCell>> = table
        .grid
        .iter()
        .map(|row| {
            let mut cells: Vec<RenderedCell> = row
                .iter()
                .map(|slot| match slot {
                    CellSlot::Origin(cell) => {
                        RenderedCell { text: render_cell_gfm(cell, rc), covered_span: false }
                    }
                    CellSlot::Covered { .. } => {
                        RenderedCell { text: String::new(), covered_span: true }
                    }
                })
                .collect();
            cells.resize_with(width, || RenderedCell { text: String::new(), covered_span: false });
            cells
        })
        .collect();
    while rendered.len() > 1
        && rendered.last().is_some_and(|r| r.iter().all(|c| c.text.is_empty() && !c.covered_span))
    {
        rendered.pop();
    }
    let width = rendered
        .iter()
        .map(|r| r.iter().rposition(|c| !c.text.is_empty() || c.covered_span).map_or(0, |i| i + 1))
        .max()
        .unwrap_or(0);
    if width == 0 {
        return None;
    }
    for row in &mut rendered {
        row.truncate(width);
    }

    let mut out = String::new();
    // Fork (sznb): GFM pipe tables have no headerless form, so the first row
    // is the header — never a synthetic empty header row (the original
    // pipeline does the same; an inserted `| | | |` line pushed the real
    // header into the data rows).
    let header: Vec<String> = if !rendered.is_empty() {
        rendered.remove(0).into_iter().map(|c| c.text).collect()
    } else {
        vec![String::new(); width]
    };
    out.push_str(&format_row(header.iter().map(String::as_str)));
    out.push('\n');
    out.push_str(&format_row(std::iter::repeat_n("---", width)));
    for row in &rendered {
        out.push('\n');
        out.push_str(&format_row(row.iter().map(|c| c.text.as_str())));
    }
    Some(out)
}

fn format_row<'a>(cells: impl IntoIterator<Item = &'a str>) -> String {
    let mut s = String::from("|");
    for cell in cells {
        s.push(' ');
        s.push_str(cell);
        s.push_str(" |");
    }
    s
}

/// Flatten arbitrary block content into a single table-cell line (GFM form).
pub(crate) fn render_cell_gfm(cell: &Cell, rc: &Ctx) -> String {
    let mut parts: Vec<String> = Vec::new();
    for block in &cell.blocks {
        cell_block_text(block, rc, &mut parts);
    }
    // Fork (sznb): pipe-table cells join their interior line breaks with a
    // space (the original pipeline's form — `数据记录 （万条）`), never
    // `<br>`; edge whitespace is padding the table's own padding would
    // swallow anyway, and spreadsheets carry it by the thousand.
    parts
        .join(" ")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
}

fn cell_block_text(block: &Block, rc: &Ctx, parts: &mut Vec<String>) {
    match block {
        Block::Heading { content, .. } => {
            let t = render_inlines(content, InlineContext::TableCell, rc);
            if !t.trim().is_empty() {
                // Fork (sznb): no `**` around heading cells.
                parts.push(t.trim().to_string());
            }
        }
        Block::Paragraph(inlines) => {
            // Edge whitespace is preserved here and protected in
            // `render_cell`; sources that retain cell padding (spreadsheets,
            // CSV) keep it in the final output.
            let t = render_inlines(inlines, InlineContext::TableCell, rc);
            if !t.trim().is_empty() {
                parts.push(t);
            }
        }
        Block::List(list) => {
            for (i, item) in list.items.iter().enumerate() {
                let mut inner: Vec<String> = Vec::new();
                for b in &item.blocks {
                    cell_block_text(b, rc, &mut inner);
                }
                let marker = match (&item.marker_label, list.marker) {
                    (Some(label), _) => format!(
                        "{} ",
                        crate::render::markdown::escape_marker_label(
                            label,
                            InlineContext::TableCell
                        )
                    ),
                    (None, crate::model::MarkerKind::Bullet) => "• ".to_string(),
                    (None, kind) => {
                        format!("{} ", kind.label(list.start.saturating_add(i as u64)))
                    }
                };
                if !inner.is_empty() {
                    parts.push(format!("{marker}{}", inner.join(" ")));
                }
            }
        }
        Block::Table(table) => {
            // Keep empty cells in the join so values stay in their source
            // column positions.
            for row in &table.grid {
                let cells: Vec<String> = row
                    .iter()
                    .map(|slot| match slot {
                        CellSlot::Origin(cell) => render_cell_gfm(cell, rc),
                        CellSlot::Covered { .. } => String::new(),
                    })
                    .collect();
                if cells.iter().any(|c| !c.is_empty()) {
                    parts.push(cells.join(" / "));
                }
            }
        }
        Block::BlockQuote(blocks) => {
            for b in blocks {
                cell_block_text(b, rc, parts);
            }
        }
        Block::CodeBlock { text, .. } => {
            let t = text.trim();
            if !t.is_empty() {
                let mut s = String::new();
                push_code_span(t, InlineContext::TableCell, &mut s);
                parts.push(s);
            }
        }
        Block::Math(tex) => {
            if !tex.trim().is_empty() {
                let mut s = String::new();
                push_math_span(tex, InlineContext::TableCell, &mut s);
                parts.push(s);
            }
        }
        Block::Rule => {}
    }
}
