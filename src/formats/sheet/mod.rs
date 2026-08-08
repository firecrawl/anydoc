//! Excel spreadsheets (xlsx, xlsm, xlsb, xls) via calamine.

mod numfmt;
mod styles;

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

pub fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let mut workbook =
        contained("workbook open", || open_workbook_auto_from_rs(Cursor::new(bytes)))?
            .map_err(map_open_error)?;
    // Number formats apply to OOXML workbooks only (xlsx, xlsm, xlam all
    // load as `Sheets::Xlsx`); xls/xlsb/ods keep raw rendering.
    let formats = match &workbook {
        Sheets::Xlsx(_) => styles::CellFormats::from_ooxml(bytes)?,
        _ => None,
    };
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
                let code = formats
                    .as_ref()
                    .and_then(|f| f.code(name, start.0 + r as u32, start.1 + c as u32));
                let text = format_data(data, code);
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

fn format_data(data: &Data, code: Option<&str>) -> String {
    match data {
        Data::Empty => String::new(),
        // Untrimmed: leading/trailing whitespace in a cell is source content.
        Data::String(s) => clean_text(s),
        Data::Float(f) => match code.and_then(|c| numfmt::render(*f, c)) {
            Some(rendered) => rendered,
            None => format_float(*f),
        },
        Data::Int(i) => match code.and_then(|c| {
            // Integers beyond f64's exact range must keep their exact
            // digits; the format path only applies when the round trip is
            // lossless (Excel itself displays 15 significant digits).
            let f = *i as f64;
            (f as i64 == *i).then(|| numfmt::render(f, c)).flatten()
        }) {
            Some(rendered) => rendered,
            None => i.to_string(),
        },
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
pub(crate) mod test_util {
    use std::io::Write;

    /// Minimal xlsx package with one sheet named `S`: workbook/rels/sheet
    /// parts, plus `styles.xml` when `styles` is `Some`. Deterministic ZIP
    /// timestamps (pattern: the `xlsx_with_merge` helper in this module).
    pub fn xlsx_with(styles: Option<&str>, sheet: &str) -> Vec<u8> {
        xlsx_with_sheets(styles, &[("S", sheet)])
    }

    /// Minimal xlsx package with `(sheet name, sheet XML)` pairs; the sheet
    /// parts are `xl/worksheets/sheet1.xml`, `sheet2.xml`, ... in order.
    pub fn xlsx_with_sheets(styles: Option<&str>, sheets: &[(&str, &str)]) -> Vec<u8> {
        let sheets_xml: Vec<String> = sheets
            .iter()
            .enumerate()
            .map(|(i, (name, _))| {
                format!(r#"<sheet name="{name}" sheetId="{}" r:id="rId{}"/>"#, i + 1, i + 1)
            })
            .collect();
        let rels_xml: Vec<String> = sheets
            .iter()
            .enumerate()
            .map(|(i, _)| {
                format!(
                    r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{}.xml"/>"#,
                    i + 1,
                    i + 1
                )
            })
            .collect();
        let content_overrides: String = sheets
            .iter()
            .enumerate()
            .map(|(i, _)| {
                format!(
                    r#"<Override PartName="/xl/worksheets/sheet{}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#,
                    i + 1
                )
            })
            .collect();
        let mut parts: Vec<(String, String)> = vec![
            (
                "[Content_Types].xml".to_string(),
                format!(
                    r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>{content_overrides}</Types>"#
                ),
            ),
            (
                "_rels/.rels".to_string(),
                r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#
                    .to_string(),
            ),
            (
                "xl/workbook.xml".to_string(),
                format!(
                    r#"<?xml version="1.0"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{}</sheets></workbook>"#,
                    sheets_xml.join("")
                ),
            ),
            (
                "xl/_rels/workbook.xml.rels".to_string(),
                format!(
                    r#"<?xml version="1.0"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}</Relationships>"#,
                    rels_xml.join("")
                ),
            ),
        ];
        for (i, (_, sheet)) in sheets.iter().enumerate() {
            parts.push((format!("xl/worksheets/sheet{}.xml", i + 1), sheet.to_string()));
        }
        if let Some(styles) = styles {
            parts.push(("xl/styles.xml".to_string(), styles.to_string()));
        }
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (name, body) in parts {
            w.start_file(&name, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal xlsx with a used range at D11:E12 and the given merged region.
    fn xlsx_with_merge(merge_ref: &str) -> Vec<u8> {
        let sheet = format!(
            r#"<?xml version="1.0"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="11"><c r="D11" t="inlineStr"><is><t>x</t></is></c><c r="E11" t="inlineStr"><is><t>y</t></is></c></row><row r="12"><c r="D12" t="inlineStr"><is><t>z</t></is></c><c r="E12" t="inlineStr"><is><t>w</t></is></c></row></sheetData><mergeCells count="1"><mergeCell ref="{merge_ref}"/></mergeCells></worksheet>"#
        );
        super::test_util::xlsx_with(None, &sheet)
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

    #[test]
    fn string_cells_are_not_trimmed() {
        assert_eq!(format_data(&Data::String("  padded  ".into()), None), "  padded  ");
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

    fn first_table_text(doc: &Document) -> Vec<String> {
        let Block::Table(t) = doc.blocks.first().unwrap() else {
            panic!("expected a table");
        };
        t.grid
            .iter()
            .flatten()
            .map(|slot| match slot {
                crate::model::CellSlot::Origin(c) => {
                    c.blocks.first().map(cell_text).unwrap_or_default()
                }
                _ => String::new(),
            })
            .collect()
    }

    /// Concatenated text of a block's inline content (cells hold one
    /// paragraph of plain text in this pipeline).
    fn cell_text(block: &Block) -> String {
        match block {
            Block::Paragraph(inlines) => crate::model::inlines_to_plain_text(inlines),
            _ => String::new(),
        }
    }

    #[test]
    fn formatted_numeric_cells_render_their_display_value() {
        let styles = r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="0.00%"/></numFmts><cellXfs count="3"><xf numFmtId="0"/><xf numFmtId="164"/><xf numFmtId="4"/></cellXfs></styleSheet>"#;
        let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="1"><v>0.65</v></c><c r="B1" s="2"><v>1234.5</v></c><c r="C1"><v>0.075</v></c></row></sheetData></worksheet>"#;
        let doc = parse(&super::test_util::xlsx_with(Some(styles), sheet)).unwrap();
        assert_eq!(first_table_text(&doc), vec!["65.00%", "1,234.50", "0.075"]);
    }

    #[test]
    fn formatted_int_cells_group_digits() {
        let styles = r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="3"/></cellXfs></styleSheet>"#;
        let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="1"><v>1234567</v></c></row></sheetData></worksheet>"#;
        let doc = parse(&super::test_util::xlsx_with(Some(styles), sheet)).unwrap();
        assert_eq!(first_table_text(&doc), vec!["1,234,567"]);
    }

    #[test]
    fn workbook_without_styles_keeps_raw_rendering() {
        let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>0.65</v></c></row></sheetData></worksheet>"#;
        let doc = parse(&super::test_util::xlsx_with(None, sheet)).unwrap();
        assert_eq!(first_table_text(&doc), vec!["0.65"]);
    }

    #[test]
    fn general_formatted_float_keeps_raw_rendering() {
        let styles = r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="1"><xf numFmtId="0"/></cellXfs></styleSheet>"#;
        let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="0"><v>0.65</v></c></row></sheetData></worksheet>"#;
        let doc = parse(&super::test_util::xlsx_with(Some(styles), sheet)).unwrap();
        assert_eq!(first_table_text(&doc), vec!["0.65"]);
    }

    #[test]
    fn date_cells_still_render_through_the_datetime_path() {
        // numFmtId 14 is a date format: calamine converts the cell to
        // DateTime and the numeric pipeline must not double-format it.
        let styles = r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="14"/></cellXfs></styleSheet>"#;
        let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="1"><v>46031</v></c></row></sheetData></worksheet>"#;
        let doc = parse(&super::test_util::xlsx_with(Some(styles), sheet)).unwrap();
        let text = first_table_text(&doc);
        assert_eq!(text, vec!["2026-01-09"], "date serial must render as a date, got {text:?}");
    }

    #[test]
    fn string_cells_ignore_format_codes() {
        let styles = r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="164"/></cellXfs></styleSheet>"#;
        let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="1" t="inlineStr"><is><t>5.75%</t></is></c></row></sheetData></worksheet>"#;
        let doc = parse(&super::test_util::xlsx_with(Some(styles), sheet)).unwrap();
        assert_eq!(first_table_text(&doc), vec!["5.75%"]);
    }

    #[test]
    fn formatted_cells_align_when_the_used_range_does_not_start_at_a1() {
        // Data starting at D11: the absolute coordinate from the cell ref
        // (D11 -> (10, 3)) must match calamine's range start offset.
        let styles = r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="9"/></cellXfs></styleSheet>"#;
        let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="11"><c r="D11" s="1"><v>0.5</v></c><c r="E11" t="inlineStr"><is><t>y</t></is></c></row><row r="12"><c r="D12" t="inlineStr"><is><t>z</t></is></c><c r="E12" t="inlineStr"><is><t>w</t></is></c></row></sheetData></worksheet>"#;
        let doc = parse(&super::test_util::xlsx_with(Some(styles), sheet)).unwrap();
        let cells = first_table_text(&doc);
        assert_eq!(cells[0], "50%", "D11 must render its format, got {cells:?}");
        assert_eq!(cells[1], "y");
        assert_eq!(cells[2], "z");
        assert_eq!(cells[3], "w");
    }

    #[test]
    fn large_integers_follow_calamine_precision() {
        // calamine parses values beyond f64's exact range as Float, so the
        // 15-significant-digit display (Excel parity) applies; a format code
        // renders that value rather than failing. The Data::Int path keeps
        // its own exactness guard (see format_data_uses_the_format_code).
        let styles = r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="3"/></cellXfs></styleSheet>"#;
        let sheet = r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="1"><v>9007199254740993</v></c><c r="B1" s="0"><v>9007199254740993</v></c></row></sheetData></worksheet>"#;
        let doc = parse(&super::test_util::xlsx_with(Some(styles), sheet)).unwrap();
        assert_eq!(first_table_text(&doc), vec!["9,007,199,254,740,992", "9007199254740990"]);
    }

    #[test]
    fn format_data_uses_the_format_code_when_present() {
        assert_eq!(format_data(&Data::Float(0.653035934239184), Some("0%")), "65%");
        assert_eq!(format_data(&Data::Float(0.653035934239184), None), "0.653035934239184");
        assert_eq!(format_data(&Data::Int(1234567), Some("#,##0")), "1,234,567");
        assert_eq!(format_data(&Data::Int(1234567), None), "1234567");
        // Integers beyond f64's exact range keep their exact digits: the
        // format path only applies when the i64 -> f64 round trip is lossless.
        let big = 9_007_199_254_740_993i64;
        assert_eq!(format_data(&Data::Int(big), Some("#,##0")), "9007199254740993");
    }
}
