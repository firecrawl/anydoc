//! ODF tables: canonical grid construction with covered-cell consumption,
//! repeats honored in every container, a hard expansion budget for
//! repeat expansion, and typed value-attribute fallback for cells without
//! display text.
//!
//! Empty filler runs *inside* the used range materialize fully so every
//! later cell keeps its source coordinates; only trailing filler (empty rows
//! at the end, empty cells at a row's end) is elided. All materialization is
//! charged against the fixed expansion budget.

use crate::error::ConvertError;
use crate::formats::odf::text::{Ctx, parse_container};
use crate::model::{Block, Cell, GridBuilder, Inline, TableKind};
use crate::package::limits;
use crate::package::xml::{Element, ns};
use crate::shared::header::resolve_header_rows;

pub fn parse_table(elem: &Element, ctx: &Ctx) -> Result<Vec<Block>, ConvertError> {
    let mut state = TableState {
        builder: GridBuilder::new(),
        expansion: 0,
        expansion_bytes: 0,
        pending_rows: 0,
        header_rows: 0,
        rows_emitted: 0,
    };
    walk_rows(elem, ctx, &mut state, true)?;
    let mut table = state.builder.finish(TableKind::Data);
    if table.grid.is_empty() {
        return Ok(Vec::new());
    }
    table.header_rows = resolve_header_rows(&table, state.header_rows);
    Ok(vec![Block::Table(table)])
}

struct TableState {
    builder: GridBuilder,
    /// Grid slots produced by expansion, checked against the fixed budget.
    expansion: u64,
    /// Text bytes duplicated by expansion, checked against their own budget.
    expansion_bytes: u64,
    pending_rows: u64,
    header_rows: usize,
    rows_emitted: usize,
}

impl TableState {
    fn charge(&mut self, cells: u64) -> Result<(), ConvertError> {
        self.expansion = self.expansion.saturating_add(cells);
        if self.expansion > limits::MAX_EXPANSION {
            return Err(ConvertError::ResourceLimit {
                limit: "max_expansion",
                detail: "table repeat expansion exceeds the content budget".into(),
            });
        }
        Ok(())
    }

    /// Charge text bytes that repeat expansion duplicates: the slot budget
    /// bounds positions, this bounds the amplified content itself.
    fn charge_bytes(&mut self, bytes: u64) -> Result<(), ConvertError> {
        self.expansion_bytes = self.expansion_bytes.saturating_add(bytes);
        if self.expansion_bytes > limits::MAX_EXPANSION_TEXT_BYTES {
            return Err(ConvertError::ResourceLimit {
                limit: "max_expansion_text_bytes",
                detail: "table repeat expansion duplicates more text than the budget".into(),
            });
        }
        Ok(())
    }
}

/// One parsed cell of a row template, reused across row repeats.
enum RowCell {
    Covered { repeat: u64 },
    Cell { repeat: u64, col_span: u32, row_span: u32, blocks: Vec<Block>, bytes: u64 },
}

/// Approximate retained text bytes of parsed cell content.
fn block_bytes(blocks: &[Block]) -> u64 {
    fn inline_bytes(inlines: &[crate::model::Inline]) -> u64 {
        use crate::model::Inline;
        inlines
            .iter()
            .map(|i| match i {
                Inline::Text { text, .. } => text.len() as u64,
                Inline::Link { content, target } => {
                    let target_len = match target {
                        crate::model::LinkTarget::External(s)
                        | crate::model::LinkTarget::Relative(s)
                        | crate::model::LinkTarget::Anchor(s) => s.len() as u64,
                    };
                    inline_bytes(content) + target_len
                }
                Inline::Image { alt, .. } => alt.len() as u64,
                Inline::Anchor(id) | Inline::NoteRef(id) => id.len() as u64,
                Inline::LineBreak => 1,
            })
            .sum()
    }
    blocks
        .iter()
        .map(|b| match b {
            Block::Paragraph(i) | Block::Heading { content: i, .. } => inline_bytes(i),
            Block::List(list) => {
                list.items.iter().map(|item| block_bytes(&item.blocks)).sum::<u64>()
            }
            Block::Table(t) => t
                .grid
                .iter()
                .flatten()
                .map(|slot| match slot {
                    crate::model::CellSlot::Origin(c) => block_bytes(&c.blocks),
                    crate::model::CellSlot::Covered { .. } => 0,
                })
                .sum(),
            Block::BlockQuote(blocks) => block_bytes(blocks),
            Block::CodeBlock { text, .. } => text.len() as u64,
            Block::Rule => 0,
        })
        .sum()
}

fn walk_rows(
    container: &Element,
    ctx: &Ctx,
    state: &mut TableState,
    top: bool,
) -> Result<(), ConvertError> {
    for child in container.child_elems() {
        if child.ns.as_deref().is_none_or(|n| n != ns::TABLE) {
            continue;
        }
        match child.local.as_str() {
            "table-header-rows" => {
                let before = state.rows_emitted;
                walk_rows(child, ctx, state, false)?;
                if top && state.header_rows == 0 {
                    state.header_rows = state.rows_emitted - before;
                }
            }
            "table-rows" | "table-row-group" => walk_rows(child, ctx, state, false)?,
            "table-row" => {
                let repeat: u64 = child
                    .attr(ns::TABLE, "number-rows-repeated")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1)
                    .max(1);
                emit_row(child, ctx, state, repeat)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn emit_row(
    row: &Element,
    ctx: &Ctx,
    state: &mut TableState,
    repeat: u64,
) -> Result<(), ConvertError> {
    if row_is_empty(row) {
        state.pending_rows = state.pending_rows.saturating_add(repeat);
        return Ok(());
    }
    // Materialize any buffered empty gap first, in full: rows after the gap
    // keep their source coordinates. The rows are charged so a pathological
    // gap hits the budget instead of memory.
    if state.pending_rows > 0 {
        state.charge(state.pending_rows)?;
        for _ in 0..state.pending_rows {
            state.builder.next_row();
            state.rows_emitted += 1;
        }
        state.pending_rows = 0;
    }
    // Parse the row template exactly once: repeated rows clone the parsed
    // cells instead of reparsing, so per-parse side effects (notes, assets)
    // happen once and the duplicated text bytes are charged up front.
    let cells = parse_row_cells(row, ctx)?;
    state.charge(repeat.saturating_sub(1))?;
    for cell in &cells {
        if let RowCell::Cell { repeat: cell_repeat, bytes, .. } = cell {
            let copies = repeat.saturating_mul(*cell_repeat).saturating_sub(1);
            state.charge_bytes(bytes.saturating_mul(copies))?;
        }
    }
    for _ in 0..repeat {
        state.builder.next_row();
        state.rows_emitted += 1;
        emit_parsed_cells(&cells, state)?;
    }
    Ok(())
}

/// Parse one row's cells into the reusable template.
fn parse_row_cells(row: &Element, ctx: &Ctx) -> Result<Vec<RowCell>, ConvertError> {
    let mut out = Vec::new();
    for cell in row.child_elems() {
        let repeat: u64 = cell
            .attr(ns::TABLE, "number-columns-repeated")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1)
            .max(1);
        if cell.is(ns::TABLE, "covered-table-cell") {
            out.push(RowCell::Covered { repeat });
            continue;
        }
        if !cell.is(ns::TABLE, "table-cell") {
            continue;
        }
        let col_span: u32 = cell
            .attr(ns::TABLE, "number-columns-spanned")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1)
            .max(1);
        let row_span: u32 = cell
            .attr(ns::TABLE, "number-rows-spanned")
            .and_then(|v| v.parse().ok())
            .unwrap_or(1)
            .max(1);
        let blocks = cell_blocks(cell, ctx)?;
        let bytes = block_bytes(&blocks);
        out.push(RowCell::Cell { repeat, col_span, row_span, blocks, bytes });
    }
    Ok(out)
}

fn emit_parsed_cells(cells: &[RowCell], state: &mut TableState) -> Result<(), ConvertError> {
    let mut pending_cells: u64 = 0;
    for cell in cells {
        match cell {
            RowCell::Covered { repeat } => {
                flush_gap(state, &mut pending_cells)?;
                // One explicitly written covered position each; a stray one
                // (no span accounts for it) becomes an empty cell inside
                // covered().
                state.charge(*repeat)?;
                for _ in 0..*repeat {
                    if !state.builder.covered() {
                        log::debug!("covered table cell without a spanning origin");
                    }
                }
            }
            RowCell::Cell { repeat, col_span, row_span, blocks, .. } => {
                if blocks.is_empty() && *col_span == 1 && *row_span == 1 {
                    pending_cells = pending_cells.saturating_add(*repeat);
                    continue;
                }
                flush_gap(state, &mut pending_cells)?;
                state.charge(repeat.saturating_mul(*col_span as u64))?;
                for _ in 0..*repeat {
                    state.builder.place(Cell::spanning(blocks.clone(), *col_span, *row_span))?;
                }
            }
        }
    }
    Ok(())
}

/// Materialize a buffered empty-cell run in full so the next cell lands on
/// its source column. Trailing runs are never flushed and stay elided.
fn flush_gap(state: &mut TableState, pending: &mut u64) -> Result<(), ConvertError> {
    if *pending == 0 {
        return Ok(());
    }
    state.charge(*pending)?;
    for _ in 0..*pending {
        state.builder.place(Cell::default())?;
    }
    *pending = 0;
    Ok(())
}

/// A cell's blocks: its text content, or a typed value-attribute fallback
/// when the producer wrote no display text.
fn cell_blocks(cell: &Element, ctx: &Ctx) -> Result<Vec<Block>, ConvertError> {
    let blocks = parse_container(cell, ctx)?;
    let has_content = blocks.iter().any(|b| match b {
        Block::Paragraph(inlines) => !crate::model::inlines_are_empty(inlines),
        _ => true,
    });
    if has_content {
        return Ok(blocks);
    }
    match value_text(cell) {
        Some(text) => Ok(vec![Block::Paragraph(vec![Inline::plain(text)])]),
        None => Ok(Vec::new()),
    }
}

fn value_text(cell: &Element) -> Option<String> {
    let value_type = cell.attr(ns::OFFICE, "value-type")?;
    match value_type {
        "percentage" => {
            let v: f64 = cell.attr(ns::OFFICE, "value")?.parse().ok()?;
            Some(format!("{}%", trim_float(v * 100.0)))
        }
        "currency" => {
            let v = cell.attr(ns::OFFICE, "value")?;
            let cur = cell.attr(ns::OFFICE, "currency").unwrap_or("");
            Some(if cur.is_empty() {
                trim_float(v.parse().ok()?)
            } else {
                format!("{} {cur}", trim_float(v.parse().ok()?))
            })
        }
        "float" => Some(trim_float(cell.attr(ns::OFFICE, "value")?.parse().ok()?)),
        "date" => cell.attr(ns::OFFICE, "date-value").map(str::to_string),
        "time" => cell.attr(ns::OFFICE, "time-value").map(duration_text),
        "boolean" => cell
            .attr(ns::OFFICE, "boolean-value")
            .map(|b| if b == "true" { "TRUE" } else { "FALSE" }.to_string()),
        "string" => cell.attr(ns::OFFICE, "string-value").map(str::to_string),
        _ => None,
    }
}

/// Shortest round-trip float formatting - no fixed-precision rounding.
fn trim_float(v: f64) -> String {
    format!("{v}")
}

/// Full ISO 8601 duration -> clock text: `P1DT2H` -> `26:00:00`,
/// `PT26H30M15S` -> `26:30:15`, weeks are 7 days, and fractional values are
/// valid on any component (`PT1.5H` -> `1:30:00`). Year/month components
/// have no fixed length, and unrepresentably large values overflow the
/// clock format, so both keep their raw ISO text.
fn duration_text(iso: &str) -> String {
    let mut total = 0.0f64; // seconds
    let mut number = String::new();
    let mut in_time = false;
    let mut negative = false;
    for c in iso.chars() {
        match c {
            '-' if total == 0.0 && number.is_empty() => negative = true,
            'P' => {}
            'T' => {
                in_time = true;
                number.clear();
            }
            'Y' | 'M' if !in_time => {
                if number.parse::<f64>().is_ok_and(|n| n != 0.0) {
                    return iso.to_string();
                }
                number.clear();
            }
            'W' | 'D' if !in_time => {
                let unit = if c == 'W' { 604_800.0 } else { 86_400.0 };
                total += number.parse::<f64>().unwrap_or(0.0) * unit;
                number.clear();
            }
            'H' | 'M' | 'S' if in_time => {
                let unit = match c {
                    'H' => 3600.0,
                    'M' => 60.0,
                    _ => 1.0,
                };
                total += number.parse::<f64>().unwrap_or(0.0) * unit;
                number.clear();
            }
            c if c.is_ascii_digit() || c == '.' => number.push(c),
            _ => number.clear(),
        }
    }
    // Round to milliseconds; a value the clock format cannot represent
    // exactly keeps its source text instead of silently degrading.
    let total_ms = (total * 1000.0).round();
    if !(0.0..1e18).contains(&total_ms) {
        return iso.to_string();
    }
    let total_ms = total_ms as u64;
    let sign = if negative { "-" } else { "" };
    let (hours, rest) = (total_ms / 3_600_000, total_ms % 3_600_000);
    let (minutes, ms) = (rest / 60_000, rest % 60_000);
    if ms % 1000 != 0 {
        format!("{sign}{hours}:{minutes:02}:{:06.3}", ms as f64 / 1000.0)
    } else {
        format!("{sign}{hours}:{minutes:02}:{:02}", ms / 1000)
    }
}

fn row_is_empty(row: &Element) -> bool {
    row.child_elems().all(|cell| {
        if cell.is(ns::TABLE, "covered-table-cell") {
            return false; // structural: belongs to a span
        }
        if !cell.is(ns::TABLE, "table-cell") {
            return true;
        }
        cell.attr(ns::TABLE, "number-columns-spanned").is_none()
            && cell.attr(ns::TABLE, "number-rows-spanned").is_none()
            && cell.attr(ns::OFFICE, "value-type").is_none()
            && cell.children.is_empty()
    })
}

/// One sheet of a spreadsheet body.
pub fn parse_spreadsheet(sheet: &Element, ctx: &Ctx) -> Result<Vec<Block>, ConvertError> {
    let tables: Vec<&Element> = sheet.child_elems().filter(|e| e.is(ns::TABLE, "table")).collect();
    let multi_sheet = tables.len() > 1;
    let mut blocks = Vec::new();
    for table in tables {
        let name = table.attr(ns::TABLE, "name").unwrap_or("");
        let content = parse_table(table, ctx)?;
        if content.is_empty() {
            continue;
        }
        if multi_sheet {
            blocks.push(Block::heading(2, vec![Inline::plain(name)]));
        }
        blocks.extend(content);
    }
    Ok(blocks)
}
