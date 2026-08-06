//! Excel spreadsheets (xlsx, xlsm, xlsb, xls) via calamine.

use crate::error::ConvertError;
use crate::model::{Block, Cell, Document, GridBuilder, Inline, TableKind};
use crate::package::limits;
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

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
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
            // A region participates only if it intersects the used range: a
            // region wholly above or left of the range must not saturate
            // onto relative (0,0). The clamped top-left decides the origin.
            let (row0, col0) = (d.start.0.max(start.0), d.start.1.max(start.1));
            let clipped_row_end = (d.end.0 as u64 + 1).min(start.0 as u64 + height as u64);
            let clipped_col_end = (d.end.1 as u64 + 1).min(start.1 as u64 + width as u64);
            if (row0 as u64) >= clipped_row_end || (col0 as u64) >= clipped_col_end {
                continue;
            }
            // The span keeps the region's full extent past the used range: a
            // merge anchored on populated cells but reaching into empty rows
            // or columns is source structure and must not silently collapse
            // to the populated rectangle. A region whose full extent would
            // blow the expansion budget (a crafted or whole-column merge)
            // degrades to today's clipped form instead — the file still
            // converts, it just loses the overhang.
            let r0 = (row0 - start.0) as usize;
            let c0 = (col0 - start.1) as usize;
            let full_row_end = d.end.0 as u64 + 1 - start.0 as u64;
            let full_col_end = d.end.1 as u64 + 1 - start.1 as u64;
            let area = (full_row_end - r0 as u64).saturating_mul(full_col_end - c0 as u64);
            let (r1, c1) = if area <= limits::MAX_EXPANSION {
                (full_row_end as usize, full_col_end as usize)
            } else {
                (
                    (clipped_row_end - start.0 as u64) as usize,
                    (clipped_col_end - start.1 as u64) as usize,
                )
            };
            if r1 - r0 == 1 && c1 - c0 == 1 {
                continue;
            }
            origins.insert((r0, c0), ((c1 - c0) as u32, (r1 - r0) as u32));
            // Covered markers are consumed only inside the used range (the
            // cell loop below); positions past it are materialized by
            // `finish`, so the set stays bounded by the cells that exist.
            for r in r0..r1.min(height) {
                for c in c0..c1.min(width) {
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
        // Merges reaching below the used range need rows to land in;
        // `finish` materializes their covered positions there.
        let deepest = origins.iter().map(|(&(r, _), &(_, rs))| r + rs as usize).max().unwrap_or(0);
        for _ in height..deepest {
            builder.next_row();
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

/// Merged regions per sheet, where the container format exposes them (xlsx
/// via each worksheet's mergeCells part, xls via BIFF MERGEDCELLS).
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
            Sheets::Xls(x) => {
                contained("merged-cell listing", || x.merge_cells_by_sheet_name(name))?
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

/// Float formatting at the 15 significant decimal digits a spreadsheet
/// stores and displays. Shortest round-trip formatting past that surfaces the
/// binary representation (`3554.7000000000003`); 15 digits still keeps small
/// values like 0.0000004 exact.
fn format_float(f: f64) -> String {
    match format!("{f:.14e}").parse::<f64>() {
        Ok(rounded) => format!("{rounded}"),
        Err(_) => format!("{f}"),
    }
}

/// Render a time-of-day serial (a fraction of a day) as `hh:mm:ss`.
fn format_time_of_day(days: f64) -> String {
    let total_secs = (days.abs() * 86_400.0).round() as u64 % 86_400;
    let (h, m, s) = (total_secs / 3600, (total_secs % 3600) / 60, total_secs % 60);
    format!("{h:02}:{m:02}:{s:02}")
}

/// Render an Excel duration (stored in days) as `[h]:mm:ss`.
fn format_duration_days(days: f64) -> String {
    let sign = if days < 0.0 { "-" } else { "" };
    let total_secs = (days.abs() * 86_400.0).round() as u64;
    let (h, m, s) = (total_secs / 3600, (total_secs % 3600) / 60, total_secs % 60);
    format!("{sign}{h}:{m:02}:{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Minimal xlsx with a used range at D11:E12 and the given merged region.
    fn xlsx_with_merge(merge_ref: &str) -> Vec<u8> {
        let sheet = format!(
            r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="11"><c r="D11" t="inlineStr"><is><t>x</t></is></c><c r="E11" t="inlineStr"><is><t>y</t></is></c></row><row r="12"><c r="D12" t="inlineStr"><is><t>z</t></is></c><c r="E12" t="inlineStr"><is><t>w</t></is></c></row></sheetData><mergeCells count="1"><mergeCell ref="{merge_ref}"/></mergeCells></worksheet>"#
        );
        xlsx_with_sheet(&sheet)
    }

    /// Minimal xlsx wrapping the given worksheet XML.
    fn xlsx_with_sheet(sheet: &str) -> Vec<u8> {
        let parts: &[(&str, &str)] = &[
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
            ),
            (
                "_rels/.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#,
            ),
            (
                "xl/workbook.xml",
                r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="S" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
            ),
            (
                "xl/_rels/workbook.xml.rels",
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#,
            ),
        ];
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in parts {
            w.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.start_file("xl/worksheets/sheet1.xml", zip::write::SimpleFileOptions::default()).unwrap();
        w.write_all(sheet.as_bytes()).unwrap();
        w.finish().unwrap().into_inner()
    }

    fn covered_count(doc: &Document) -> usize {
        let Some(Block::Table(t)) = doc.blocks.first() else {
            panic!("expected a table, got {:?}", doc.blocks.first());
        };
        t.grid
            .iter()
            .flatten()
            .filter(|s| matches!(s, crate::model::CellSlot::Covered { .. }))
            .count()
    }

    #[test]
    fn merge_inside_the_used_range_covers_cells() {
        // Harness sanity: an in-range merge must actually load and apply.
        let doc = parse(&xlsx_with_merge("D11:E11")).unwrap();
        assert_eq!(covered_count(&doc), 1);
    }

    #[test]
    fn merge_outside_the_used_columns_is_ignored() {
        // M6: the merge overlaps the used rows but not the used columns; the
        // old relative saturation mapped it onto (0,0) and covered D12.
        let doc = parse(&xlsx_with_merge("A1:B12")).unwrap();
        assert_eq!(covered_count(&doc), 0, "out-of-range merge must not cover cells");
    }

    fn first_table(doc: &Document) -> &crate::model::Table {
        let Some(Block::Table(t)) = doc.blocks.first() else {
            panic!("expected a table, got {:?}", doc.blocks.first());
        };
        t
    }

    #[test]
    fn merge_extending_past_the_used_range_keeps_its_span() {
        // A merge anchored on the used range but reaching into empty rows
        // and columns (D11:H15 over a D11:E12 used range) must keep its full
        // 5x5 extent instead of clipping to the populated 2x2.
        let doc = parse(&xlsx_with_merge("D11:H15")).unwrap();
        let t = first_table(&doc);
        assert_eq!((t.grid.len(), t.grid[0].len()), (5, 5));
        assert!(
            matches!(&t.grid[0][0], crate::model::CellSlot::Origin(c)
                if c.row_span == 5 && c.col_span == 5),
            "span must cover the whole merge: {:?}",
            t.grid[0][0]
        );
        assert_eq!(covered_count(&doc), 24);
    }

    #[test]
    fn merge_anchored_on_the_only_populated_cell_keeps_its_span() {
        // The issue-8 shape: the sole populated cell anchors the merge, so
        // the used range is 1x1 and the old intersection collapsed the merge
        // to an ordinary cell.
        let sheet = r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="F1" t="inlineStr"><is><t>Merged heading</t></is></c></row></sheetData><mergeCells count="1"><mergeCell ref="F1:O3"/></mergeCells></worksheet>"#;
        let doc = parse(&xlsx_with_sheet(sheet)).unwrap();
        let t = first_table(&doc);
        assert_eq!((t.grid.len(), t.grid[0].len()), (3, 10));
        assert!(matches!(&t.grid[0][0], crate::model::CellSlot::Origin(c)
            if c.row_span == 3 && c.col_span == 10));
        assert_eq!(covered_count(&doc), 29);
    }

    #[test]
    fn oversized_merge_degrades_to_the_clipped_form() {
        // A crafted whole-sheet merge must not materialize a huge grid, but
        // the file must still convert: the region degrades to the populated
        // range (here 1x1) instead of erroring.
        let sheet = r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>x</t></is></c></row></sheetData><mergeCells count="1"><mergeCell ref="A1:XFD1048576"/></mergeCells></worksheet>"#;
        let doc = parse(&xlsx_with_sheet(sheet)).unwrap();
        let t = first_table(&doc);
        assert_eq!((t.grid.len(), t.grid[0].len()), (1, 1));
        assert!(matches!(&t.grid[0][0], crate::model::CellSlot::Origin(c)
            if c.row_span == 1 && c.col_span == 1));
    }

    #[test]
    fn string_cells_are_not_trimmed() {
        assert_eq!(format_data(&Data::String("  padded  ".into())), "  padded  ");
    }

    #[test]
    fn tiny_floats_survive() {
        assert_eq!(format_float(0.0000004), "0.0000004");
        assert_eq!(format_float(12.0), "12");
        assert_eq!(format_float(1.5), "1.5");
    }

    #[test]
    fn time_of_day_serials_carry_no_date() {
        // 09:04:54 as a fraction of a day, with the float noise a serial
        // carries in practice.
        assert_eq!(format_time_of_day(32_694.184 / 86_400.0), "09:04:54");
        assert_eq!(format_time_of_day(0.0), "00:00:00");
    }

    #[test]
    fn floats_render_at_spreadsheet_precision() {
        assert_eq!(format_float(3554.7000000000003), "3554.7");
        assert_eq!(format_float(5649.5599999999995), "5649.56");
        assert_eq!(format_float(346_289_529.491_800_1), "346289529.4918");
        // Small values stay exact: 15 significant digits reaches far below 1.
        assert_eq!(format_float(0.0000004), "0.0000004");
        assert_eq!(format_float(1.0), "1");
    }

    #[test]
    fn durations_render_as_clock_time() {
        // 26h30m15s = 1.104340277... days
        let days = (26.0 * 3600.0 + 30.0 * 60.0 + 15.0) / 86_400.0;
        assert_eq!(format_duration_days(days), "26:30:15");
        assert_eq!(format_duration_days(-0.5), "-12:00:00");
    }
}
