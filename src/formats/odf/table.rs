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
use crate::model::{
    Block, Cell, CellSlot, GridBuilder, Inline, SpreadsheetRange, SpreadsheetSource, Table,
    TableKind,
};
use crate::package::limits;
use crate::package::xml::{Element, ns};
use crate::shared::header::resolve_header_rows;
use crate::shared::text::clean_text;
use std::collections::HashMap;

pub fn parse_table(elem: &Element, ctx: &Ctx) -> Result<Vec<Block>, ConvertError> {
    parse_table_with_source(elem, ctx, None)
}

fn parse_table_with_source(
    elem: &Element,
    ctx: &Ctx,
    source: Option<(u32, &str)>,
) -> Result<Vec<Block>, ConvertError> {
    let mut state = TableState {
        builder: GridBuilder::new(),
        expansion: 0,
        expansion_bytes: 0,
        pending_rows: 0,
        header_rows: 0,
        rows_emitted: 0,
        next_source_row: 0,
        checkboxes: read_checkboxes(elem),
    };
    // A spreadsheet merge is real source extent even when its final rows
    // contain only covered cells. Ordinary ODF tables keep the historical
    // trailing-covered-row trim, so enable this only for spreadsheet tables.
    if source.is_some() {
        state.builder.keep_covered_tail();
    }
    walk_rows(elem, ctx, &mut state, true)?;
    let mut table = state.builder.finish(TableKind::Data);
    if table.grid.is_empty() {
        return Ok(Vec::new());
    }
    table.header_rows = resolve_header_rows(&table, state.header_rows);
    if let Some((sheet_index, sheet_name)) = source
        && let Some(range) = table_source_range(&table)
    {
        table.source =
            Some(SpreadsheetSource { sheet_index, sheet_name: sheet_name.to_string(), range });
    }
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
    /// Source row of the next row element, including rows buffered as empty.
    next_source_row: u64,
    /// The sheet's form checkboxes by control id, as the inlines a
    /// `draw:control` in a cell expands to.
    checkboxes: HashMap<String, Vec<Inline>>,
}

/// Checkbox controls declared in the table's `office:forms`. The mixed
/// ("unknown") state has no token and is left out.
fn read_checkboxes(table: &Element) -> HashMap<String, Vec<Inline>> {
    let mut out = HashMap::new();
    for cb in table.find_all(ns::OFFICE, "forms").flat_map(|f| f.descendants(ns::FORM, "checkbox"))
    {
        let Some(id) = cb.attr_qualified(ns::XML, "id").or_else(|| cb.attr(ns::FORM, "id")) else {
            continue;
        };
        let state = cb.attr(ns::FORM, "current-state").or_else(|| cb.attr(ns::FORM, "state"));
        let checked = match state {
            Some("checked") => true,
            Some("unchecked") | None => false,
            Some(_) => continue,
        };
        let mut inlines = vec![Inline::Checkbox(checked)];
        let label = cb.attr(ns::FORM, "label").map(clean_text).unwrap_or_default();
        if !label.is_empty() {
            inlines.push(Inline::plain(format!(" {label}")));
        }
        out.insert(id.to_string(), inlines);
    }
    out
}

/// Append the checkboxes anchored in a cell after its content, on the same
/// line. Controls under `table:shapes` are anchored to the page by
/// coordinates alone, so they are not placed.
fn append_cell_checkboxes(
    cell: &Element,
    checkboxes: &HashMap<String, Vec<Inline>>,
    blocks: &mut Vec<Block>,
) {
    for control in cell.find_all(ns::DRAW, "control") {
        let Some(found) = control.attr(ns::DRAW, "control").and_then(|id| checkboxes.get(id))
        else {
            continue;
        };
        match blocks.last_mut() {
            Some(Block::Paragraph(inlines)) => {
                inlines.push(Inline::plain(" "));
                inlines.extend(found.iter().cloned());
            }
            _ => blocks.push(Block::Paragraph(found.clone())),
        }
    }
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
                Inline::Math(tex) => tex.len() as u64,
                Inline::Anchor(id) | Inline::NoteRef(id) => id.len() as u64,
                Inline::Checkbox(_) => 3,
                Inline::LineBreak => 1,
            })
            .sum()
    }
    blocks
        .iter()
        .map(|b| match b {
            Block::Paragraph(i) | Block::Heading { content: i, .. } => inline_bytes(i),
            Block::Math(tex) => tex.len() as u64,
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
    let source_row = state.next_source_row;
    state.next_source_row = state.next_source_row.saturating_add(repeat);
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
    let cells = parse_row_cells(row, ctx, &state.checkboxes)?;
    state.charge(repeat.saturating_sub(1))?;
    for cell in &cells {
        if let RowCell::Cell { repeat: cell_repeat, bytes, .. } = cell {
            let copies = repeat.saturating_mul(*cell_repeat).saturating_sub(1);
            state.charge_bytes(bytes.saturating_mul(copies))?;
        }
    }
    for row_offset in 0..repeat {
        let emitted_source_row = source_row.saturating_add(row_offset);
        state.builder.next_row();
        state.rows_emitted += 1;
        emit_parsed_cells(&cells, state, emitted_source_row)?;
    }
    Ok(())
}

/// Parse one row's cells into the reusable template.
fn parse_row_cells(
    row: &Element,
    ctx: &Ctx,
    checkboxes: &HashMap<String, Vec<Inline>>,
) -> Result<Vec<RowCell>, ConvertError> {
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
        let mut blocks = cell_blocks(cell, ctx)?;
        append_cell_checkboxes(cell, checkboxes, &mut blocks);
        let bytes = block_bytes(&blocks);
        out.push(RowCell::Cell { repeat, col_span, row_span, blocks, bytes });
    }
    Ok(out)
}

fn emit_parsed_cells(
    cells: &[RowCell],
    state: &mut TableState,
    source_row: u64,
) -> Result<(), ConvertError> {
    let mut pending_cells: u64 = 0;
    let mut source_col: u64 = 0;
    for cell in cells {
        match cell {
            RowCell::Covered { repeat } => {
                let gap_start = source_col.saturating_sub(pending_cells);
                flush_gap(state, &mut pending_cells, source_row, gap_start)?;
                // One explicitly written covered position each; a stray one
                // (no span accounts for it) becomes an empty cell inside
                // covered().
                state.charge(*repeat)?;
                for _ in 0..*repeat {
                    let source = source_cell(source_row, source_col)?;
                    if !state.builder.covered_with(Cell { source: Some(source), ..Cell::default() })
                    {
                        log::debug!("covered table cell without a spanning origin");
                    }
                    source_col = source_col.saturating_add(1);
                }
            }
            RowCell::Cell { repeat, col_span, row_span, blocks, .. } => {
                if blocks.is_empty() && *col_span == 1 && *row_span == 1 {
                    pending_cells = pending_cells.saturating_add(*repeat);
                    source_col = source_col.saturating_add(*repeat);
                    continue;
                }
                let gap_start = source_col.saturating_sub(pending_cells);
                flush_gap(state, &mut pending_cells, source_row, gap_start)?;
                state.charge(repeat.saturating_mul(*col_span as u64))?;
                for _ in 0..*repeat {
                    let end_row = source_row.saturating_add(u64::from(*row_span).saturating_sub(1));
                    let end_col = source_col.saturating_add(u64::from(*col_span).saturating_sub(1));
                    let source = source_range(source_row, source_col, end_row, end_col)?;
                    let mut origin = Cell::spanning(blocks.clone(), *col_span, *row_span);
                    origin.source = Some(source);
                    state.builder.place(origin)?;
                    source_col = source_col.saturating_add(u64::from(*col_span));
                }
            }
        }
    }
    Ok(())
}

/// Materialize a buffered empty-cell run in full so the next cell lands on
/// its source column. Trailing runs are never flushed and stay elided.
fn flush_gap(
    state: &mut TableState,
    pending: &mut u64,
    source_row: u64,
    source_col: u64,
) -> Result<(), ConvertError> {
    if *pending == 0 {
        return Ok(());
    }
    state.charge(*pending)?;
    for offset in 0..*pending {
        let source = source_cell(source_row, source_col.saturating_add(offset))?;
        state.builder.place(Cell { source: Some(source), ..Cell::default() })?;
    }
    *pending = 0;
    Ok(())
}

fn source_cell(row: u64, column: u64) -> Result<SpreadsheetRange, ConvertError> {
    source_range(row, column, row, column)
}

fn source_range(
    start_row: u64,
    start_column: u64,
    end_row: u64,
    end_column: u64,
) -> Result<SpreadsheetRange, ConvertError> {
    let to_u32 = |value: u64, axis: &str| {
        u32::try_from(value).map_err(|_| ConvertError::ResourceLimit {
            limit: "spreadsheet_coordinates",
            detail: format!("{axis} coordinate {value} exceeds the model range"),
        })
    };
    Ok(SpreadsheetRange::new(
        to_u32(start_row, "row")?,
        to_u32(start_column, "column")?,
        to_u32(end_row, "row")?,
        to_u32(end_column, "column")?,
    ))
}

fn table_source_range(table: &Table) -> Option<SpreadsheetRange> {
    let mut range: Option<SpreadsheetRange> = None;
    for row in &table.grid {
        for slot in row {
            let CellSlot::Origin(cell) = slot else {
                continue;
            };
            let Some(source) = cell.source else {
                continue;
            };
            match &mut range {
                Some(found) => found.include(source),
                None => range = Some(source),
            }
        }
    }
    range
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
    for (sheet_index, table) in tables.into_iter().enumerate() {
        let name = table.attr(ns::TABLE, "name").unwrap_or("");
        let content = parse_table_with_source(table, ctx, Some((sheet_index as u32, name)))?;
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
