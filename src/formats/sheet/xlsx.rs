//! In-house SpreadsheetML reader (.xlsx / .xlsm): the workbook's visible
//! sheets, shared strings, cell number formats from `xl/styles.xml`, and
//! merge regions. Rows, columns, and sheets the source hides are omitted,
//! and merge regions are remapped onto the surviving grid.

use super::controls::{Checkboxes, cell_inlines, read_vml_checkboxes};
use super::numfmt::{DateParts, NumberFormat, Rendered, builtin_code};
use super::{format_duration_days, format_float, format_time_of_day};
use crate::error::ConvertError;
use crate::model::{
    Block, Cell, Document, GridBuilder, Inline, SpreadsheetRange, SpreadsheetSource, Table,
    TableKind,
};
use crate::package::limits;
use crate::package::relationships::{Relationships, read_rels, rel_type, rels_part_for};
use crate::package::xml::{Element, ns};
use crate::package::{Package, path};
use crate::shared::header::resolve_header_rows;
use crate::shared::text::clean_text;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub(super) const SHARED_STRINGS_REL: &str =
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings";

/// The grid bounds the format defines; a reference outside them is not a
/// real cell.
pub(super) const MAX_ROWS: u32 = 1_048_576;
pub(super) const MAX_COLS: u32 = 16_384;

pub(super) fn parse(pkg: &mut Package, wb_part: &str) -> Result<Document, ConvertError> {
    let workbook = pkg.required_xml_part(wb_part)?;
    // Any OOXML main part resolves here; a document or presentation would
    // otherwise convert to an empty workbook.
    if !workbook.child_elems().next().is_some_and(|e| e.is(ns::SML, "workbook")) {
        return Err(ConvertError::malformed("main part is not a workbook"));
    }
    let wb_rels = read_rels(pkg, &rels_part_for(wb_part))?;
    let date1904 = workbook
        .first_descendant(ns::SML, "workbookPr")
        .and_then(|e| e.attr_unqualified("date1904"))
        .is_some_and(bool_attr);

    let shared =
        match sibling_part(pkg, &wb_rels, wb_part, SHARED_STRINGS_REL, "sharedStrings.xml")? {
            Some(root) => shared_strings(&root),
            None => Vec::new(),
        };
    let styles =
        Styles::read(sibling_part(pkg, &wb_rels, wb_part, rel_type::STYLES, "styles.xml")?);

    // Visible sheets in workbook order; hidden and veryHidden sheets are
    // omitted entirely, heading included.
    let mut sheets: Vec<(u32, String, String)> = Vec::new();
    for (sheet_index, sheet) in workbook
        .first_descendant(ns::SML, "sheets")
        .into_iter()
        .flat_map(|s| s.find_all(ns::SML, "sheet"))
        .enumerate()
    {
        if matches!(sheet.attr_unqualified("state"), Some("hidden" | "veryHidden")) {
            continue;
        }
        let name = sheet.attr_unqualified("name").unwrap_or_default().to_string();
        let Some(target) =
            sheet.attr_qualified(ns::R, "id").and_then(|rid| wb_rels.internal_target(rid))
        else {
            log::warn!("skipping sheet {name:?} with no worksheet relationship");
            continue;
        };
        match path::resolve(wb_part, target) {
            Ok(t) => sheets.push((sheet_index as u32, name, t.path)),
            Err(e) => log::warn!("skipping sheet {name:?} with unresolvable target: {e}"),
        }
    }

    let multi_sheet = sheets.len() > 1;
    let mut doc = Document::default();
    let mut failed = 0usize;
    // One budget for the workbook, so sheets cannot multiply the cap.
    let mut slots = 0u64;
    for (sheet_index, name, part) in &sheets {
        let worksheet = pkg.optional_xml_part(part)?;
        let Some(worksheet) = worksheet.as_ref().and_then(|r| r.find(ns::SML, "worksheet")) else {
            log::warn!("skipping unreadable sheet {name:?}");
            failed += 1;
            continue;
        };
        let mut content = read_sheet(worksheet, &shared, &styles, date1904);
        content.checkboxes = read_vml_checkboxes(pkg, part)?;
        let Some(table) = build_table(content, *sheet_index, name, &mut slots)? else {
            continue;
        };
        if multi_sheet {
            doc.blocks.push(Block::heading(2, vec![Inline::plain(name.clone())]));
        }
        doc.blocks.push(Block::Table(table));
    }
    if !sheets.is_empty() && failed == sheets.len() {
        return Err(ConvertError::malformed("no sheet in the workbook could be read"));
    }
    Ok(doc)
}

/// Part name for a workbook-level sibling: the relationship of the given
/// type when present, else the conventional name next to the workbook part.
pub(super) fn sibling_part_name(
    rels: &Relationships,
    base: &str,
    rel: &str,
    conventional: &str,
) -> String {
    rels.first_of_type(rel)
        .and_then(|r| path::resolve(base, &r.target).ok())
        .map(|t| t.path)
        .unwrap_or_else(|| match base.rsplit_once('/') {
            Some((dir, _)) => format!("{dir}/{conventional}"),
            None => conventional.to_string(),
        })
}

/// Load a workbook-level XML part by relationship type, falling back to the
/// conventional name next to the workbook part.
fn sibling_part(
    pkg: &mut Package,
    rels: &Relationships,
    base: &str,
    rel: &str,
    conventional: &str,
) -> Result<Option<Element>, ConvertError> {
    pkg.optional_xml_part(&sibling_part_name(rels, base, rel, conventional))
}

/// The shared string table, one cleaned entry per `si` in order.
fn shared_strings(root: &Element) -> Vec<String> {
    let Some(sst) = root.find(ns::SML, "sst") else {
        return Vec::new();
    };
    sst.find_all(ns::SML, "si").map(|si| clean_text(&rich_text(si))).collect()
}

/// Text of an `si` or `is`: a single `t`, or rich-text `r` runs
/// concatenated. Phonetic guides (`rPh`) are not content.
fn rich_text(item: &Element) -> String {
    let mut out = String::new();
    for child in item.child_elems() {
        if child.is(ns::SML, "t") {
            out.push_str(&child.text());
        } else if child.is(ns::SML, "r")
            && let Some(t) = child.find(ns::SML, "t")
        {
            out.push_str(&t.text());
        }
    }
    out
}

/// A cell's resolved number format: General, or a parsed format code.
/// Shared with the BIFF reader - .xls XF/FORMAT records resolve into the
/// same representation.
#[derive(Clone)]
pub(super) enum CellFormat {
    General,
    Fmt(Rc<NumberFormat>),
}

/// `xl/styles.xml` reduced to what rendering needs: the ordered `cellXfs`
/// list, each entry's numFmtId resolved to a parsed format.
struct Styles {
    xfs: Vec<CellFormat>,
}

impl Styles {
    fn read(root: Option<Element>) -> Styles {
        let Some(root) = root else {
            return Styles { xfs: Vec::new() };
        };
        let mut custom: HashMap<u32, &str> = HashMap::new();
        for fmts in root.descendants(ns::SML, "numFmts") {
            for nf in fmts.find_all(ns::SML, "numFmt") {
                if let (Some(id), Some(code)) = (
                    nf.attr_unqualified("numFmtId").and_then(|v| v.parse().ok()),
                    nf.attr_unqualified("formatCode"),
                ) {
                    custom.insert(id, code);
                }
            }
        }
        let mut cache: HashMap<u32, CellFormat> = HashMap::new();
        let xfs = root
            .first_descendant(ns::SML, "cellXfs")
            .map(|xfs| {
                xfs.find_all(ns::SML, "xf")
                    .map(|xf| {
                        let id = xf
                            .attr_unqualified("numFmtId")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0u32);
                        cache.entry(id).or_insert_with(|| resolve_format(id, &custom)).clone()
                    })
                    .collect()
            })
            .unwrap_or_default();
        Styles { xfs }
    }

    /// The format for a cell's `s` attribute, an index into `cellXfs`
    /// (default 0).
    fn for_cell(&self, s: Option<&str>) -> &CellFormat {
        let i = s.and_then(|s| s.parse::<usize>().ok()).unwrap_or(0);
        self.xfs.get(i).unwrap_or(&CellFormat::General)
    }
}

/// A numFmtId's format: the file's own `numFmt` entries first, then the
/// built-in table. Unknown ids and unsupported codes fall back to General -
/// never to a guess.
pub(super) fn resolve_format(id: u32, custom: &HashMap<u32, &str>) -> CellFormat {
    let code = custom.get(&id).copied().or_else(|| builtin_code(id));
    match code {
        Some(code) => match NumberFormat::parse(code) {
            Some(f) => CellFormat::Fmt(Rc::new(f)),
            None => {
                log::debug!("unsupported number format {code:?}, rendering as General");
                CellFormat::General
            }
        },
        None => {
            if id != 0 {
                log::debug!("numFmtId {id} has no resolvable code, rendering as General");
            }
            CellFormat::General
        }
    }
}

/// One worksheet, parsed but not yet filtered or gridded. Shared with the
/// BIFF reader, which fills it from records instead of XML.
#[derive(Default)]
pub(super) struct SheetContent {
    /// Rendered text by zero-based (row, col); empty results are absent.
    pub(super) cells: HashMap<(u32, u32), String>,
    /// Form control checkboxes by the cell they are anchored in.
    pub(super) checkboxes: Checkboxes,
    pub(super) hidden_rows: HashSet<u32>,
    /// Inclusive zero-based column ranges hidden by `cols/col` entries.
    pub(super) hidden_cols: Vec<(u32, u32)>,
    /// Inclusive zero-based merge regions (r1, c1, r2, c2), area > 1.
    pub(super) merges: Vec<(u32, u32, u32, u32)>,
}

fn read_sheet(
    worksheet: &Element,
    shared: &[String],
    styles: &Styles,
    date1904: bool,
) -> SheetContent {
    let mut out = SheetContent::default();
    for cols in worksheet.find_all(ns::SML, "cols") {
        for col in cols.find_all(ns::SML, "col") {
            if !col.attr_unqualified("hidden").is_some_and(bool_attr) {
                continue;
            }
            let bound = |name| {
                col.attr_unqualified(name)
                    .and_then(|v| v.parse::<u32>().ok())
                    .and_then(|v| v.checked_sub(1))
            };
            if let (Some(min), Some(max)) = (bound("min"), bound("max"))
                && min <= max
            {
                out.hidden_cols.push((min, max.min(MAX_COLS - 1)));
            }
        }
    }
    let mut next_row: u32 = 0;
    for row in worksheet.find_all(ns::SML, "sheetData").flat_map(|sd| sd.find_all(ns::SML, "row")) {
        let r = row
            .attr_unqualified("r")
            .and_then(|v| v.parse::<u32>().ok())
            .and_then(|v| v.checked_sub(1))
            .unwrap_or(next_row);
        if r >= MAX_ROWS {
            continue;
        }
        next_row = r + 1;
        if row.attr_unqualified("hidden").is_some_and(bool_attr) {
            out.hidden_rows.insert(r);
        }
        let mut next_col: u32 = 0;
        for c in row.find_all(ns::SML, "c") {
            // Position comes from the cell's own reference: a row may skip
            // cells entirely, so iteration order says nothing.
            let (cr, cc) = match c.attr_unqualified("r").map(parse_ref) {
                Some(Some(rc)) => rc,
                Some(None) => continue,
                None => (r, next_col),
            };
            next_col = cc + 1;
            if cr >= MAX_ROWS || cc >= MAX_COLS {
                continue;
            }
            let text = cell_text(c, shared, styles, date1904);
            if !text.is_empty() {
                out.cells.insert((cr, cc), text);
            }
        }
    }
    for merge in
        worksheet.find_all(ns::SML, "mergeCells").flat_map(|mc| mc.find_all(ns::SML, "mergeCell"))
    {
        let Some(region) = merge.attr_unqualified("ref").and_then(parse_region) else {
            log::debug!("skipping unparseable merge reference");
            continue;
        };
        let (r1, c1, r2, c2) = region;
        if r1 != r2 || c1 != c2 {
            out.merges.push(region);
        }
    }
    out
}

/// A cell's rendered text, per its `t` type and resolved number format.
fn cell_text(c: &Element, shared: &[String], styles: &Styles, date1904: bool) -> String {
    let fmt = styles.for_cell(c.attr_unqualified("s"));
    let value = || c.find(ns::SML, "v").map(|v| v.text()).unwrap_or_default();
    match c.attr_unqualified("t").unwrap_or("n") {
        "s" => {
            let v = value();
            match v.trim().parse::<usize>().ok().and_then(|i| shared.get(i)) {
                Some(text) => format_as_text(fmt, text),
                None => {
                    log::debug!("shared string index {v:?} out of range");
                    String::new()
                }
            }
        }
        "str" => format_as_text(fmt, &clean_text(&value())),
        "inlineStr" => {
            let text = c.find(ns::SML, "is").map(|is| clean_text(&rich_text(is)));
            format_as_text(fmt, &text.unwrap_or_default())
        }
        "b" => match value().trim() {
            "1" | "true" => "TRUE".to_string(),
            "0" | "false" => "FALSE".to_string(),
            _ => String::new(),
        },
        "e" | "d" => clean_text(&value()),
        _ => {
            let v = value();
            let v = v.trim();
            if v.is_empty() {
                return String::new();
            }
            let Ok(n) = v.parse::<f64>() else {
                log::debug!("unparseable numeric cell value {v:?}");
                return String::new();
            };
            render_number(fmt, n, date1904)
        }
    }
}

/// Render a numeric cell through its resolved format. Shared with the BIFF
/// and binary readers so every container formats identically.
pub(super) fn render_number(fmt: &CellFormat, n: f64, date1904: bool) -> String {
    let text = match fmt {
        CellFormat::General => format_float(n),
        CellFormat::Fmt(f) => match f.format_number(n) {
            Rendered::General { value, prefix, suffix } => {
                format!("{prefix}{}{suffix}", format_float(value))
            }
            Rendered::Text(s) => s,
            Rendered::DateTime(parts) => render_serial(n, parts, date1904),
        },
    };
    clean_text(&text)
}

pub(super) fn format_as_text(fmt: &CellFormat, text: &str) -> String {
    match fmt {
        CellFormat::Fmt(f) => match f.format_text(text) {
            Some(s) => clean_text(&s),
            None => text.to_string(),
        },
        CellFormat::General => text.to_string(),
    }
}

/// Materialize a sheet's grid: visibility filtering, the populated extent
/// widened to cover intersecting merge regions (a merge anchored on the
/// only populated cell must survive at full size), and merges remapped onto
/// the surviving rows and columns.
pub(super) fn build_table(
    mut sheet: SheetContent,
    sheet_index: u32,
    sheet_name: &str,
    slots: &mut u64,
) -> Result<Option<Table>, ConvertError> {
    // Hidden coordinates as sorted lists: lookups and first-visible scans
    // stay logarithmic, so an adversarial pile of hidden rows or column
    // ranges cannot force quadratic work.
    let hidden_rows = {
        let mut rows: Vec<u32> = sheet.hidden_rows.iter().copied().collect();
        rows.sort_unstable();
        rows
    };
    let hidden_cols = expand_ranges(&mut sheet.hidden_cols);
    let hidden_row = |r: u32| hidden_rows.binary_search(&r).is_ok();
    let hidden_col = |c: u32| hidden_cols.binary_search(&c).is_ok();

    // Cell text and anchored checkboxes become one inline map; the rest of
    // the assembly no longer cares which was which.
    let mut cells: HashMap<(u32, u32), Vec<Inline>> = HashMap::new();
    for (at, boxes) in sheet.checkboxes.drain() {
        if at.0 < MAX_ROWS && at.1 < MAX_COLS {
            cells.insert(at, cell_inlines(sheet.cells.remove(&at), &boxes));
        }
    }
    for (at, text) in sheet.cells.drain() {
        cells.insert(at, vec![Inline::plain(text)]);
    }
    // A merge with no surviving row or column disappears with its content.
    // One whose origin is hidden keeps its content at the first surviving
    // position it covers, so the value is not lost.
    sheet.merges.retain(|&(r1, c1, r2, c2)| {
        let vr = first_visible(&hidden_rows, r1, r2);
        let vc = first_visible(&hidden_cols, c1, c2);
        let (Some(vr), Some(vc)) = (vr, vc) else {
            return false;
        };
        if (vr, vc) != (r1, c1)
            && let Some(text) = cells.remove(&(r1, c1))
        {
            cells.insert((vr, vc), text);
        }
        true
    });

    // Populated extent over visible cells only.
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for &(r, c) in cells.keys() {
        if hidden_row(r) || hidden_col(c) {
            continue;
        }
        bounds = Some(match bounds {
            None => (r, c, r, c),
            Some((r1, c1, r2, c2)) => (r1.min(r), c1.min(c), r2.max(r), c2.max(c)),
        });
    }
    let Some((mut r1, mut c1, mut r2, mut c2)) = bounds else {
        return Ok(None);
    };
    // Merge regions touching the populated extent widen it to their full
    // size; the rest are dropped, so a crafted merge list can neither force
    // unbounded materialization nor saturate onto (0,0).
    sheet.merges.retain(|&(mr1, mc1, mr2, mc2)| mr1 <= r2 && mr2 >= r1 && mc1 <= c2 && mc2 >= c1);
    for &(mr1, mc1, mr2, mc2) in &sheet.merges {
        (r1, c1, r2, c2) = (r1.min(mr1), c1.min(mc1), r2.max(mr2), c2.max(mc2));
    }

    let row_map: Vec<u32> = (r1..=r2).filter(|&r| !hidden_row(r)).collect();
    let col_map: Vec<u32> = (c1..=c2).filter(|&c| !hidden_col(c)).collect();
    if row_map.is_empty() || col_map.is_empty() {
        return Ok(None);
    }
    // Charged before materializing, and across the workbook rather than per
    // sheet: the extent comes from cell coordinates, so a handful of cells
    // describes a whole sheet and a handful of sheets multiplies it.
    *slots = slots.saturating_add(row_map.len() as u64 * col_map.len() as u64);
    if *slots > limits::MAX_GRID_SLOTS {
        return Err(ConvertError::ResourceLimit {
            limit: "max_grid_slots",
            detail: format!("workbook extent covers {slots} grid positions"),
        });
    }

    // Remap merges onto the surviving coordinates. The covered-position set
    // is charged against the expansion budget up front, before any
    // insertion work, mirroring what placement would charge.
    let visible_span = |map: &[u32], lo: u32, hi: u32| {
        let a = map.partition_point(|&x| x < lo);
        let b = map.partition_point(|&x| x <= hi);
        (a, b - a)
    };
    let mut origins: HashMap<(usize, usize), MergeOrigin> = HashMap::new();
    let mut covered: HashSet<(usize, usize)> = HashSet::new();
    let mut expansion = 0u64;
    for &(mr1, mc1, mr2, mc2) in &sheet.merges {
        let (r0, rn) = visible_span(&row_map, mr1, mr2);
        let (c0, cn) = visible_span(&col_map, mc1, mc2);
        if rn == 0 || cn == 0 {
            continue;
        }
        let source = SpreadsheetRange::new(mr1, mc1, mr2, mc2);
        if rn * cn <= 1 {
            // A merge can collapse to one visible slot when its other rows or
            // columns are hidden. It no longer needs covered markers, but the
            // origin still represents the complete source merge rectangle.
            origins.insert((r0, c0), MergeOrigin { col_span: 1, row_span: 1, source });
            continue;
        }
        expansion = expansion.saturating_add((rn as u64) * (cn as u64) - 1);
        if expansion > limits::MAX_EXPANSION {
            return Err(ConvertError::ResourceLimit {
                limit: "max_expansion",
                detail: "merge region expansion exceeds the content budget".into(),
            });
        }
        origins.insert((r0, c0), MergeOrigin { col_span: cn as u32, row_span: rn as u32, source });
        for r in r0..r0 + rn {
            for c in c0..c0 + cn {
                if (r, c) != (r0, c0) {
                    covered.insert((r, c));
                }
            }
        }
    }

    let mut builder = GridBuilder::new();
    // A merge is real extent: trailing rows it covers stay in the grid.
    builder.keep_covered_tail();
    for (ri, &row) in row_map.iter().enumerate() {
        builder.next_row();
        for (ci, &col) in col_map.iter().enumerate() {
            if covered.contains(&(ri, ci)) {
                builder.covered();
                continue;
            }
            let mut cell = match cells.remove(&(row, col)) {
                Some(inlines) => Cell::from_inlines(inlines),
                None => Cell::default(),
            };
            match origins.get(&(ri, ci)) {
                Some(origin) => {
                    cell.col_span = origin.col_span;
                    cell.row_span = origin.row_span;
                    cell.source = Some(origin.source);
                }
                None => {
                    cell.source = Some(SpreadsheetRange::cell(row, col));
                }
            }
            builder.place(cell)?;
        }
    }
    // A spreadsheet marks no header row, so the shape of the data decides.
    let mut table = builder.finish(TableKind::Data);
    if table.grid.is_empty() {
        return Ok(None);
    }
    table.header_rows = resolve_header_rows(&table, 0);
    table.source = Some(SpreadsheetSource {
        sheet_index,
        sheet_name: sheet_name.to_string(),
        range: SpreadsheetRange::new(r1, c1, r2, c2),
    });
    Ok(Some(table))
}

#[derive(Clone, Copy)]
struct MergeOrigin {
    col_span: u32,
    row_span: u32,
    source: SpreadsheetRange,
}

/// Flatten inclusive ranges into a sorted, deduplicated coordinate list.
/// Coalescing before expansion bounds the output by the coordinate space,
/// not by the range count.
fn expand_ranges(ranges: &mut [(u32, u32)]) -> Vec<u32> {
    ranges.sort_unstable();
    let mut out = Vec::new();
    let mut next = 0u32;
    for &(a, b) in ranges.iter() {
        out.extend(a.max(next)..=b);
        next = next.max(b.saturating_add(1));
    }
    out
}

/// First coordinate in `lo..=hi` absent from the sorted hidden list. The
/// consecutive hidden run starting at `lo` is measured by binary search, so
/// a long run cannot force a linear scan per query.
fn first_visible(hidden: &[u32], lo: u32, hi: u32) -> Option<u32> {
    let tail = &hidden[hidden.partition_point(|&h| h < lo)..];
    // `tail[j] - j` never decreases, so "the run still holds at j" is a
    // prefix property.
    let (mut a, mut b) = (0usize, tail.len());
    while a < b {
        let mid = (a + b) / 2;
        if tail[mid] == lo + mid as u32 {
            a = mid + 1;
        } else {
            b = mid;
        }
    }
    let first = lo.checked_add(u32::try_from(a).ok()?)?;
    (first <= hi).then_some(first)
}

/// Render a date/time serial the way the crate always has: elapsed formats
/// as a duration, sub-day serials as a time of day, everything else as an
/// ISO-like date with the midnight time omitted.
fn render_serial(serial: f64, parts: DateParts, date1904: bool) -> String {
    if !serial.is_finite() {
        return format_float(serial);
    }
    if parts.elapsed {
        return format_duration_days(serial);
    }
    if !parts.date {
        return format_time_of_day(serial.fract());
    }
    // A serial carrying no whole day has no date for a date format to show;
    // the clock a combined format also names still renders.
    if serial.abs() < 1.0 {
        return if parts.time { format_time_of_day(serial) } else { format_float(serial) };
    }
    // Out of the representable date range (through 9999-12-31): the serial
    // is not a date, show the number.
    if !(0.0..2_958_466.0).contains(&serial) {
        return format_float(serial);
    }
    let mut days = serial.trunc() as i64;
    // The fictitious 1900-02-29 has no date to render and would otherwise
    // collapse onto serial 59. Tested before the seconds carry, so a value
    // late on serial 59 still resolves as the date it belongs to.
    if !date1904 && days == 60 {
        return format_float(serial);
    }
    let mut secs = (serial.fract() * 86_400.0).round() as i64;
    if secs >= 86_400 {
        secs = 0;
        days += 1;
    }
    let civil_days = if date1904 {
        days + days_from_civil(1904, 1, 1)
    } else {
        // 1900 system: serial 1 is 1900-01-01, and the fictitious
        // 1900-02-29 (serial 60) offsets everything after it by one day.
        days - i64::from(days >= 60) + days_from_civil(1899, 12, 31)
    };
    let (y, m, d) = civil_from_days(civil_days);
    if !(1..=9999).contains(&y) {
        return format_float(serial);
    }
    let mut out = format!("{y:04}-{m:02}-{d:02}");
    if parts.time && secs != 0 {
        out.push_str(&format!(" {:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60));
    }
    out
}

/// Days from 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// A civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (y + i64::from(m <= 2), m, d)
}

/// A cell reference (`C3`) as zero-based (row, column); the column letters
/// are bijective base-26.
fn parse_ref(r: &str) -> Option<(u32, u32)> {
    let digits_at = r.find(|c: char| c.is_ascii_digit())?;
    let (letters, digits) = r.split_at(digits_at);
    if letters.is_empty() {
        return None;
    }
    let mut col: u32 = 0;
    for ch in letters.chars() {
        if !ch.is_ascii_alphabetic() {
            return None;
        }
        col = col.checked_mul(26)?.checked_add(ch.to_ascii_uppercase() as u32 - 'A' as u32 + 1)?;
        if col > MAX_COLS {
            return None;
        }
    }
    let row: u32 = digits.parse().ok()?;
    if !(1..=MAX_ROWS).contains(&row) {
        return None;
    }
    Some((row - 1, col - 1))
}

/// A merge reference (`F1:O3`, or a single cell) as an inclusive normalized
/// region.
fn parse_region(r: &str) -> Option<(u32, u32, u32, u32)> {
    let (a, b) = r.split_once(':').unwrap_or((r, r));
    let (r1, c1) = parse_ref(a.trim())?;
    let (r2, c2) = parse_ref(b.trim())?;
    Some((r1.min(r2), c1.min(c2), r1.max(r2), c1.max(c2)))
}

/// XML schema boolean attribute.
fn bool_attr(v: &str) -> bool {
    matches!(v.trim(), "1" | "true")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CellSlot, SpreadsheetRange, inlines_to_plain_text};
    use std::io::Write;

    const SML: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
    const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
    const PKG_RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
    const WS_REL: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet";
    const STYLES_REL: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles";

    /// Route through the shared container dispatch, the only entry a caller
    /// has.
    fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
        super::super::parse(bytes)
    }

    /// Assemble a workbook: (name, state, worksheet body) per sheet, plus
    /// optional styleSheet and sst parts.
    #[derive(Default)]
    struct Wb<'a> {
        sheets: Vec<(&'a str, &'a str, &'a str)>,
        styles: Option<&'a str>,
        shared: Option<&'a str>,
        date1904: bool,
        /// Further parts, verbatim: (name, body).
        extra: Vec<(&'a str, &'a str)>,
    }

    impl Wb<'_> {
        fn build(&self) -> Vec<u8> {
            let mut sheets = String::new();
            let mut rels = String::new();
            for (i, (name, state, _)) in self.sheets.iter().enumerate() {
                let id = i + 1;
                let state =
                    if state.is_empty() { String::new() } else { format!(" state=\"{state}\"") };
                sheets.push_str(&format!(
                    r#"<sheet name="{name}" sheetId="{id}"{state} r:id="rId{id}"/>"#
                ));
                rels.push_str(&format!(
                    r#"<Relationship Id="rId{id}" Type="{WS_REL}" Target="worksheets/sheet{id}.xml"/>"#
                ));
            }
            if self.styles.is_some() {
                rels.push_str(&format!(
                    r#"<Relationship Id="rId90" Type="{STYLES_REL}" Target="styles.xml"/>"#
                ));
            }
            if self.shared.is_some() {
                rels.push_str(&format!(
                    r#"<Relationship Id="rId91" Type="{SHARED_STRINGS_REL}" Target="sharedStrings.xml"/>"#
                ));
            }
            let pr = if self.date1904 { r#"<workbookPr date1904="1"/>"# } else { "" };
            let workbook = format!(
                r#"<?xml version="1.0"?><workbook xmlns="{SML}" xmlns:r="{R}">{pr}<sheets>{sheets}</sheets></workbook>"#
            );
            let rels = format!(
                r#"<?xml version="1.0"?><Relationships xmlns="{PKG_RELS}">{rels}</Relationships>"#
            );
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            let opts = zip::write::SimpleFileOptions::default();
            let mut add = |name: &str, body: &str| {
                zip.start_file(name, opts).unwrap();
                zip.write_all(body.as_bytes()).unwrap();
            };
            add("xl/workbook.xml", &workbook);
            add("xl/_rels/workbook.xml.rels", &rels);
            for (i, (_, _, body)) in self.sheets.iter().enumerate() {
                add(
                    &format!("xl/worksheets/sheet{}.xml", i + 1),
                    &format!(r#"<?xml version="1.0"?><worksheet xmlns="{SML}">{body}</worksheet>"#),
                );
            }
            if let Some(styles) = self.styles {
                add(
                    "xl/styles.xml",
                    &format!(
                        r#"<?xml version="1.0"?><styleSheet xmlns="{SML}">{styles}</styleSheet>"#
                    ),
                );
            }
            if let Some(shared) = self.shared {
                add(
                    "xl/sharedStrings.xml",
                    &format!(r#"<?xml version="1.0"?><sst xmlns="{SML}">{shared}</sst>"#),
                );
            }
            for (name, body) in &self.extra {
                add(name, body);
            }
            zip.finish().unwrap().into_inner()
        }
    }

    fn one_sheet(body: &str) -> Wb<'_> {
        Wb { sheets: vec![("S", "", body)], ..Wb::default() }
    }

    fn first_table(doc: &Document) -> &Table {
        match doc.blocks.iter().find_map(|b| match b {
            Block::Table(t) => Some(t),
            _ => None,
        }) {
            Some(t) => t,
            None => panic!("expected a table, got {:?}", doc.blocks),
        }
    }

    fn texts(table: &Table) -> Vec<Vec<String>> {
        table
            .grid
            .iter()
            .map(|row| {
                row.iter()
                    .map(|slot| match slot {
                        CellSlot::Origin(cell) => cell
                            .blocks
                            .iter()
                            .filter_map(|b| match b {
                                Block::Paragraph(i) => Some(inlines_to_plain_text(i)),
                                _ => None,
                            })
                            .collect(),
                        CellSlot::Covered { .. } => "<covered>".to_string(),
                    })
                    .collect()
            })
            .collect()
    }

    fn covered_count(table: &Table) -> usize {
        table.grid.iter().flatten().filter(|s| matches!(s, CellSlot::Covered { .. })).count()
    }

    #[test]
    fn number_formats_apply_to_stored_values() {
        // The issue #27 case: a percent renders as its display value, not
        // its stored fraction; currency keeps its symbol and grouping; a
        // date format keeps the unambiguous ISO rendering.
        let wb = Wb {
            styles: Some(
                r#"<numFmts><numFmt numFmtId="164" formatCode="0.0%"/><numFmt numFmtId="165" formatCode="&quot;$&quot;#,##0.00"/><numFmt numFmtId="166" formatCode="mm/dd/yyyy"/></numFmts><cellXfs><xf numFmtId="0"/><xf numFmtId="164"/><xf numFmtId="165"/><xf numFmtId="166"/></cellXfs>"#,
            ),
            ..one_sheet(
                r#"<sheetData><row r="1"><c r="A1" s="1"><v>0.075</v></c><c r="B1" s="2"><v>1234.5</v></c><c r="C1" s="3"><v>46096</v></c></row></sheetData>"#,
            )
        };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["7.5%", "$1,234.50", "2026-03-15"]]);
    }

    #[test]
    fn unresolvable_numfmt_ids_render_general() {
        // Id 5 is not an implied built-in and id 30 is locale-specific:
        // with no numFmt element the code is unknown, and guessing (a
        // currency format, a date shape) would be worse than General.
        let wb = Wb {
            styles: Some(r#"<cellXfs><xf numFmtId="5"/><xf numFmtId="30"/></cellXfs>"#),
            ..one_sheet(
                r#"<sheetData><row r="1"><c r="A1" s="0"><v>1234.5</v></c><c r="B1" s="1"><v>1234.5</v></c></row></sheetData>"#,
            )
        };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["1234.5", "1234.5"]]);
    }

    #[test]
    fn value_types_render_by_their_t_attribute() {
        let wb = one_sheet(
            r#"<sheetData><row r="1"><c r="A1" t="b"><v>1</v></c><c r="B1" t="e"><v>#DIV/0!</v></c><c r="C1" t="str"><v>=sum</v></c><c r="D1" t="d"><v>2026-03-15</v></c><c r="E1" t="inlineStr"><is><r><t>in</t></r><r><t>line</t></r></is></c></row></sheetData>"#,
        );
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(
            texts(first_table(&doc)),
            vec![vec!["TRUE", "#DIV/0!", "=sum", "2026-03-15", "inline"]]
        );
    }

    #[test]
    fn shared_strings_resolve_including_rich_text_runs() {
        let wb = Wb {
            shared: Some(
                r#"<si><t>plain</t></si><si><r><t>ri</t></r><r><t>ch</t></r><rPh sb="0" eb="1"><t>ignored</t></rPh></si>"#,
            ),
            ..one_sheet(
                r#"<sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row></sheetData>"#,
            )
        };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["plain", "rich"]]);
    }

    #[test]
    fn date1904_serials_shift_epoch() {
        let wb = Wb {
            styles: Some(
                r#"<numFmts><numFmt numFmtId="164" formatCode="yyyy-mm-dd"/></numFmts><cellXfs><xf numFmtId="164"/></cellXfs>"#,
            ),
            date1904: true,
            ..one_sheet(r#"<sheetData><row r="1"><c r="A1" s="0"><v>100</v></c></row></sheetData>"#)
        };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["1904-04-10"]]);
    }

    #[test]
    fn merge_extends_past_the_populated_range() {
        // Issue #8: the only populated cell anchors F1:O3, so the grid must
        // widen to the merge's full 3x10 extent instead of clipping to the
        // 1x1 populated range.
        let wb = one_sheet(
            r#"<sheetData><row r="1"><c r="F1" t="inlineStr"><is><t>wide</t></is></c></row></sheetData><mergeCells count="1"><mergeCell ref="F1:O3"/></mergeCells>"#,
        );
        let doc = parse(&wb.build()).unwrap();
        let table = first_table(&doc);
        assert_eq!(table.grid.len(), 3);
        assert_eq!(table.grid[1].len(), 10);
        let CellSlot::Origin(cell) = &table.grid[0][0] else {
            panic!("expected the merge origin at (0,0)");
        };
        assert_eq!((cell.col_span, cell.row_span), (10, 3));
        assert_eq!(covered_count(table), 29);
    }

    /// Minimal sheet with a used range at D11:E12 and the given merge.
    fn sheet_with_merge(merge_ref: &str) -> Vec<u8> {
        one_sheet(&format!(
            r#"<sheetData><row r="11"><c r="D11" t="inlineStr"><is><t>x</t></is></c><c r="E11" t="inlineStr"><is><t>y</t></is></c></row><row r="12"><c r="D12" t="inlineStr"><is><t>z</t></is></c><c r="E12" t="inlineStr"><is><t>w</t></is></c></row></sheetData><mergeCells count="1"><mergeCell ref="{merge_ref}"/></mergeCells>"#
        ))
        .build()
    }

    #[test]
    fn merge_inside_the_used_range_covers_cells() {
        let doc = parse(&sheet_with_merge("D11:E11")).unwrap();
        assert_eq!(covered_count(first_table(&doc)), 1);
    }

    #[test]
    fn merge_outside_the_used_columns_is_ignored() {
        // The merge overlaps the used rows but not the used columns; it
        // must neither cover cells nor drag the grid out to column A.
        let doc = parse(&sheet_with_merge("A1:B12")).unwrap();
        let table = first_table(&doc);
        assert_eq!(covered_count(table), 0, "out-of-range merge must not cover cells");
        assert_eq!(table.grid[0].len(), 2);
    }

    #[test]
    fn hidden_rows_columns_and_sheets_are_omitted() {
        // Hidden content is invisible to someone opening the workbook, so
        // passing it on would make it look authoritative. One visible sheet
        // remains, so no sheet heading is emitted either.
        let visible = r#"<cols><col min="2" max="2" hidden="1"/></cols><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>a</t></is></c><c r="B1" t="inlineStr"><is><t>hidden col</t></is></c><c r="C1" t="inlineStr"><is><t>c</t></is></c></row><row r="2" hidden="1"><c r="A2" t="inlineStr"><is><t>hidden row</t></is></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>d</t></is></c></row></sheetData>"#;
        let secret = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>secret</t></is></c></row></sheetData>"#;
        let wb = Wb {
            sheets: vec![("Shown", "", visible), ("Secret", "hidden", secret)],
            ..Wb::default()
        };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(doc.blocks.len(), 1, "hidden sheet must add no heading and no table");
        let table = first_table(&doc);
        assert_eq!(texts(table), vec![vec!["a", "c"], vec!["d", ""]]);
        let CellSlot::Origin(cell) = &table.grid[0][0] else { panic!() };
        assert_eq!(cell.source, Some(SpreadsheetRange::cell(0, 0)));
        let CellSlot::Origin(cell) = &table.grid[0][1] else { panic!() };
        assert_eq!(cell.source, Some(SpreadsheetRange::cell(0, 2)));
        let CellSlot::Origin(cell) = &table.grid[1][0] else { panic!() };
        assert_eq!(cell.source, Some(SpreadsheetRange::cell(2, 0)));
        let CellSlot::Origin(cell) = &table.grid[1][1] else { panic!() };
        assert_eq!(cell.source, Some(SpreadsheetRange::cell(2, 2)));
    }

    #[test]
    fn source_coordinates_keep_workbook_order_and_sparse_extents() {
        let first = r#"<sheetData><row r="3"><c r="C3" t="inlineStr"><is><t>value</t></is></c></row></sheetData>"#;
        let hidden = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>hidden</t></is></c></row></sheetData>"#;
        let third = r#"<sheetData><row r="11"><c r="D11" t="inlineStr"><is><t>d</t></is></c></row><row r="12"><c r="E12" t="inlineStr"><is><t>e</t></is></c></row></sheetData>"#;
        let wb = Wb {
            sheets: vec![
                ("Data Sheet", "", first),
                ("Hidden", "hidden", hidden),
                ("Report", "", third),
            ],
            ..Wb::default()
        };
        let doc = parse(&wb.build()).unwrap();
        let tables: Vec<&Table> = doc
            .blocks
            .iter()
            .filter_map(|block| match block {
                Block::Table(table) => Some(table),
                _ => None,
            })
            .collect();
        assert_eq!(tables.len(), 2);

        let first_source = tables[0].source.as_ref().expect("spreadsheet source");
        assert_eq!(first_source.sheet_index, 0);
        assert_eq!(first_source.sheet_name, "Data Sheet");
        assert_eq!(first_source.range, SpreadsheetRange::cell(2, 2));
        let CellSlot::Origin(cell) = &tables[0].grid[0][0] else {
            panic!("expected C3 origin");
        };
        assert_eq!(cell.source, Some(SpreadsheetRange::cell(2, 2)));

        let third_source = tables[1].source.as_ref().expect("spreadsheet source");
        assert_eq!(third_source.sheet_index, 2, "hidden sheet must not renumber provenance");
        assert_eq!(third_source.sheet_name, "Report");
        assert_eq!(third_source.range, SpreadsheetRange::new(10, 3, 11, 4));
        let CellSlot::Origin(cell) = &tables[1].grid[0][0] else {
            panic!("expected D11 origin");
        };
        assert_eq!(cell.source, Some(SpreadsheetRange::cell(10, 3)));
        let CellSlot::Origin(cell) = &tables[1].grid[0][1] else {
            panic!("expected generated E11 padding");
        };
        assert_eq!(cell.source, Some(SpreadsheetRange::cell(10, 4)));
        let CellSlot::Origin(cell) = &tables[1].grid[1][1] else {
            panic!("expected E12 origin");
        };
        assert_eq!(cell.source, Some(SpreadsheetRange::cell(11, 4)));
    }

    #[test]
    fn merges_remap_across_hidden_columns() {
        // Dropping a hidden column renumbers the grid: a merge spanning it
        // comes out one column narrower, not applied at stale indices.
        let wb = one_sheet(
            r#"<cols><col min="2" max="2" hidden="1"/></cols><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>m</t></is></c><c r="D1" t="inlineStr"><is><t>x</t></is></c></row></sheetData><mergeCells count="1"><mergeCell ref="A1:C1"/></mergeCells>"#,
        );
        let doc = parse(&wb.build()).unwrap();
        let table = first_table(&doc);
        let CellSlot::Origin(cell) = &table.grid[0][0] else {
            panic!("expected the merge origin at (0,0)");
        };
        assert_eq!((cell.col_span, cell.row_span), (2, 1));
        assert_eq!(cell.source, Some(SpreadsheetRange::new(0, 0, 0, 2)));
        assert_eq!(table.grid[0].len(), 3);
    }

    #[test]
    fn collapsed_merges_keep_their_original_source_range() {
        // Both the second column and second row are hidden, so A1:B2
        // normalizes to a single visible slot. Provenance must still identify
        // the complete source merge rather than degrading to A1:A1.
        let wb = one_sheet(
            r#"<cols><col min="2" max="2" hidden="1"/></cols><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>merged</t></is></c></row><row r="2" hidden="1"/></sheetData><mergeCells count="1"><mergeCell ref="A1:B2"/></mergeCells>"#,
        );
        let doc = parse(&wb.build()).unwrap();
        let table = first_table(&doc);
        assert_eq!(table.grid.len(), 1);
        assert_eq!(table.grid[0].len(), 1);
        assert_eq!(
            table.source.as_ref().expect("spreadsheet source").range,
            SpreadsheetRange::new(0, 0, 1, 1)
        );
        let CellSlot::Origin(cell) = &table.grid[0][0] else {
            panic!("expected the collapsed merge origin");
        };
        assert_eq!((cell.col_span, cell.row_span), (1, 1));
        assert_eq!(cell.source, Some(SpreadsheetRange::new(0, 0, 1, 1)));
    }

    #[test]
    fn merge_origin_in_a_hidden_row_keeps_its_content() {
        // The origin row is hidden but the merge survives: its value moves
        // to the first surviving position it covers instead of being lost.
        let wb = one_sheet(
            r#"<sheetData><row r="1" hidden="1"><c r="A1" t="inlineStr"><is><t>kept</t></is></c></row><row r="2"/><row r="3"><c r="B3" t="inlineStr"><is><t>x</t></is></c></row></sheetData><mergeCells count="1"><mergeCell ref="A1:A3"/></mergeCells>"#,
        );
        let doc = parse(&wb.build()).unwrap();
        let table = first_table(&doc);
        let CellSlot::Origin(cell) = &table.grid[0][0] else {
            panic!("expected the merge origin at (0,0)");
        };
        assert_eq!(cell.row_span, 2);
        assert_eq!(cell.source, Some(SpreadsheetRange::new(0, 0, 2, 0)));
        assert_eq!(texts(table)[0][0], "kept");
    }

    #[test]
    fn the_grid_budget_spans_the_whole_workbook() {
        // Two cells at opposite corners describe the whole sheet, so the
        // extent has to be charged before any position is built.
        let wb = one_sheet(
            r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>a</t></is></c></row><row r="1048576"><c r="XFD1048576" t="inlineStr"><is><t>b</t></is></c></row></sheetData>"#,
        );
        let err = parse(&wb.build()).unwrap_err();
        assert!(
            matches!(err, ConvertError::ResourceLimit { limit: "max_grid_slots", .. }),
            "got {err:?}"
        );

        // Sheets accumulate, so a pile of them cannot each sit under the cap.
        let half = r#"<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>a</t></is></c></row><row r="150000"><c r="P150000" t="inlineStr"><is><t>b</t></is></c></row></sheetData>"#;
        let wb = Wb { sheets: vec![("A", "", half), ("B", "", half)], ..Wb::default() };
        let err = parse(&wb.build()).unwrap_err();
        assert!(
            matches!(err, ConvertError::ResourceLimit { limit: "max_grid_slots", .. }),
            "got {err:?}"
        );
    }

    const DATE_ONLY: DateParts = DateParts { date: true, time: false, elapsed: false };

    #[test]
    fn the_fictitious_leap_day_keeps_its_own_value() {
        // Serial 60 is 1900-02-29, a day that never existed, and mapping it
        // onto a real date would make it indistinguishable from serial 59.
        assert_eq!(render_serial(59.0, DATE_ONLY, false), "1900-02-28");
        assert_eq!(render_serial(60.0, DATE_ONLY, false), "60");
        assert_eq!(render_serial(61.0, DATE_ONLY, false), "1900-03-01");
        // A value late on serial 59 still belongs to its own day.
        assert_eq!(render_serial(59.9999999, DATE_ONLY, false), "1900-02-28");
        // The 1904 system has no such day.
        assert_eq!(render_serial(60.0, DATE_ONLY, true), "1904-03-01");
    }

    #[test]
    fn a_sub_day_serial_keeps_the_clock_a_combined_format_names() {
        let both = DateParts { date: true, time: true, elapsed: false };
        assert_eq!(render_serial(0.5, both, false), "12:00:00");
        // A date-only format still has nothing to show but the number.
        assert_eq!(render_serial(0.5, DATE_ONLY, false), "0.5");
    }

    const VML_REL: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/vmlDrawing";

    /// A legacy drawing with a checked captioned box over B1, an unchecked
    /// bare one over C1, a hidden one over D1, and a cell note.
    const VML: &str = r##"<xml xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:x="urn:schemas-microsoft-com:office:excel">
        <v:shape id="_x0000_s1025" type="#_x0000_t201" style="position:absolute;margin-left:1pt">
          <v:textbox>
            <div style="text-align:left"><font face="Tahoma">Roof</font></div>
          </v:textbox>
          <x:ClientData ObjectType="Checkbox"><x:Anchor>1, 5, 0, 2, 2, 10, 1, 1</x:Anchor><x:Checked>1</x:Checked></x:ClientData>
        </v:shape>
        <v:shape id="_x0000_s1026" type="#_x0000_t201" style="position:absolute">
          <v:textbox><div><font></font></div></v:textbox>
          <x:ClientData ObjectType="Checkbox"><x:Anchor>2, 5, 0, 2, 3, 10, 1, 1</x:Anchor></x:ClientData>
        </v:shape>
        <v:shape id="_x0000_s1027" type="#_x0000_t201" style="position:absolute;visibility:hidden">
          <x:ClientData ObjectType="Checkbox"><x:Anchor>3, 5, 0, 2, 4, 10, 1, 1</x:Anchor><x:Checked>1</x:Checked></x:ClientData>
        </v:shape>
        <v:shape id="_x0000_s1028" type="#_x0000_t202" style="position:absolute">
          <v:textbox><div>a note</div></v:textbox>
          <x:ClientData ObjectType="Note"><x:Anchor>4, 5, 0, 2, 5, 10, 1, 1</x:Anchor></x:ClientData>
        </v:shape>
        </xml>"##;

    #[test]
    fn form_control_checkboxes_land_in_their_anchor_cell() {
        let rels = format!(
            r#"<?xml version="1.0"?><Relationships xmlns="{PKG_RELS}"><Relationship Id="rId1" Type="{VML_REL}" Target="../drawings/vmlDrawing1.vml"/></Relationships>"#
        );
        let wb = Wb {
            sheets: vec![(
                "S",
                "",
                r#"<sheetData><row r="1"><c r="A1" t="str"><v>14</v></c><c r="B1" t="str"><v>L/R</v></c></row></sheetData><legacyDrawing r:id="rId1"/>"#,
            )],
            extra: vec![
                ("xl/worksheets/_rels/sheet1.xml.rels", &rels),
                ("xl/drawings/vmlDrawing1.vml", VML),
            ],
            ..Wb::default()
        };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["14", "L/R [x] Roof", "[ ]"]]);
    }
}
