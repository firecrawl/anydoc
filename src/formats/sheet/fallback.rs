//! Calamine fallback for the one Excel container the in-house readers do
//! not cover yet: binary .xlsb.

use super::{format_duration_days, format_float, format_time_of_day};
use crate::error::ConvertError;
use crate::model::{Block, Cell, Document, GridBuilder, Inline, TableKind};
use crate::shared::header::resolve_header_rows;
use crate::shared::text::clean_text;
use calamine::{Data, Dimensions, Reader, Sheets, open_workbook_auto_from_rs};
use std::collections::{HashMap, HashSet};
use std::io::Cursor;

/// Run one calamine operation behind a panic barrier: the calamine fork can
/// panic on corrupt containers (pending an upstream fix), and a dependency
/// panic must degrade to a typed error - while bugs in this crate's own code
/// stay panics. `AssertUnwindSafe` is sound here because a caught panic
/// always propagates as an error, so the workbook is never used again.
fn contained<T>(op: &str, f: impl FnOnce() -> T) -> Result<T, ConvertError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|_| {
        log::warn!("spreadsheet parser panicked during {op} on malformed input");
        ConvertError::malformed("unreadable workbook (parser aborted)")
    })
}

pub(super) fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let mut workbook =
        contained("workbook open", || open_workbook_auto_from_rs(Cursor::new(bytes)))?
            .map_err(map_open_error)?;
    let sheet_names = contained("sheet listing", || workbook.sheet_names().to_owned())?;
    let multi_sheet = sheet_names.len() > 1;
    let merged = merged_regions(&mut workbook, &sheet_names)?;

    let mut doc = Document::default();
    let mut failed = 0usize;
    for name in &sheet_names {
        let range = match contained("worksheet read", || workbook.worksheet_range(name))? {
            Ok(r) => r,
            Err(e) => {
                log::warn!("skipping unreadable sheet {name:?}: {e}");
                failed += 1;
                continue;
            }
        };
        if range.is_empty() {
            continue;
        }
        // Merged regions in range-relative coordinates: the top-left cell
        // becomes a spanning origin, the other positions are covered.
        let start = range.start().unwrap_or((0, 0));
        let (height, width) = (range.height(), range.width());
        let mut origins: HashMap<(usize, usize), (u32, u32)> = HashMap::new();
        let mut covered: HashSet<(usize, usize)> = HashSet::new();
        for d in merged.get(name.as_str()).map(Vec::as_slice).unwrap_or_default() {
            // Intersect the absolute merged region with the used range first:
            // a region wholly above or left of the range must not saturate
            // onto relative (0,0), and positions outside the range are never
            // materialized (a crafted region list must not force insertions
            // beyond the cells that actually exist).
            let (row0, col0) = (d.start.0.max(start.0), d.start.1.max(start.1));
            let row_end = (d.end.0 as u64 + 1).min(start.0 as u64 + height as u64);
            let col_end = (d.end.1 as u64 + 1).min(start.1 as u64 + width as u64);
            if (row0 as u64) >= row_end || (col0 as u64) >= col_end {
                continue;
            }
            // Translate the non-empty intersection to range-relative form.
            let r0 = (row0 - start.0) as usize;
            let c0 = (col0 - start.1) as usize;
            let r1 = (row_end - start.0 as u64) as usize;
            let c1 = (col_end - start.1 as u64) as usize;
            if r1 - r0 == 1 && c1 - c0 == 1 {
                continue;
            }
            origins.insert((r0, c0), ((c1 - c0) as u32, (r1 - r0) as u32));
            for r in r0..r1 {
                for c in c0..c1 {
                    if (r, c) != (r0, c0) {
                        covered.insert((r, c));
                    }
                }
            }
        }
        let mut builder = GridBuilder::new();
        for (r, row) in range.rows().enumerate() {
            builder.next_row();
            for (c, data) in row.iter().enumerate() {
                if covered.contains(&(r, c)) {
                    builder.covered();
                    continue;
                }
                let text = format_data(data);
                let cell = if text.is_empty() {
                    Cell::default()
                } else {
                    Cell::from_inlines(vec![Inline::plain(text)])
                };
                match origins.get(&(r, c)) {
                    Some(&(col_span, row_span)) => {
                        builder.place(Cell::spanning(cell.blocks, col_span, row_span))?
                    }
                    None => builder.place(cell)?,
                }
            }
        }
        // A spreadsheet marks no header row, so the shape of the data decides.
        let mut table = builder.finish(TableKind::Data);
        if table.grid.is_empty() {
            continue;
        }
        table.header_rows = resolve_header_rows(&table, 0);
        if multi_sheet {
            doc.blocks.push(Block::heading(2, vec![Inline::plain(name.clone())]));
        }
        doc.blocks.push(Block::Table(table));
    }
    if !sheet_names.is_empty() && failed == sheet_names.len() {
        return Err(ConvertError::malformed("no sheet in the workbook could be read"));
    }
    Ok(doc)
}

/// Merged regions per sheet, where the container format exposes them.
fn merged_regions<RS: std::io::Read + std::io::Seek>(
    workbook: &mut Sheets<RS>,
    sheet_names: &[String],
) -> Result<HashMap<String, Vec<Dimensions>>, ConvertError> {
    let mut out: HashMap<String, Vec<Dimensions>> = HashMap::new();
    for name in sheet_names {
        let regions = match workbook {
            Sheets::Xlsx(x) => {
                contained("merged-region listing", || x.merge_cells_by_sheet_name(name))?
                    .map_err(|e| e.to_string())
            }
            _ => continue,
        };
        match regions {
            Ok(dims) if !dims.is_empty() => {
                out.insert(name.clone(), dims);
            }
            Ok(_) => {}
            Err(e) => log::warn!("skipping unreadable merged-region list for {name:?}: {e}"),
        }
    }
    Ok(out)
}

fn map_open_error(e: calamine::Error) -> ConvertError {
    let text = e.to_string();
    if text.to_ascii_lowercase().contains("password") {
        ConvertError::Encrypted
    } else {
        ConvertError::malformed(format!("unreadable workbook: {text}"))
    }
}

fn format_data(data: &Data) -> String {
    match data {
        Data::Empty => String::new(),
        // Untrimmed: leading/trailing whitespace in a cell is source content.
        Data::String(s) => clean_text(s),
        Data::Float(f) => format_float(*f),
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Data::Error(e) => format!("#{e:?}"),
        Data::DateTime(dt) if dt.is_duration() => format_duration_days(dt.as_f64()),
        // A serial below one whole day carries no date: it is a time of day.
        Data::DateTime(dt) if dt.as_f64().abs() < 1.0 => format_time_of_day(dt.as_f64()),
        Data::DateTime(dt) => match dt.as_datetime() {
            Some(d) => {
                let s = d.to_string();
                // Sub-second digits are noise from the serial's float.
                let s = s.split('.').next().unwrap_or(&s);
                s.strip_suffix(" 00:00:00").unwrap_or(s).to_string()
            }
            None => format_float(dt.as_f64()),
        },
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_cells_are_not_trimmed() {
        assert_eq!(format_data(&Data::String("  padded  ".into())), "  padded  ");
    }
}
