//! OOXML (xlsx/xlsm) cell number-format metadata reader.
//!
//! calamine exposes cell values but no per-cell format codes, so the format
//! pipeline reads the workbook's own parts: `xl/styles.xml` (custom `numFmts`
//! and the `cellXfs` style table), `xl/workbook.xml` + its relationships
//! (sheet name -> part path), and each worksheet part (cell `s` style index,
//! cell `r` reference). Cells map to format codes keyed by absolute
//! `(row, col)`, aligned with calamine's used range by subtracting the range
//! start at the call site.

use crate::error::ConvertError;
use crate::package::archive::Package;
use crate::package::relationships::{read_rels, rels_part_for};
use crate::package::xml::ns::{PKG_RELS, R};
use crate::package::xml::{parse_xml, Element};
use std::collections::HashMap;
use std::rc::Rc;

/// SpreadsheetML main namespace.
const X: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";

/// Per-cell format codes, keyed by sheet name then absolute (row, col).
#[derive(Debug, Default)]
pub struct CellFormats {
    by_sheet: HashMap<String, HashMap<(u32, u32), String>>,
}

impl CellFormats {
    /// Read format metadata from an OOXML workbook.
    ///
    /// `Ok(None)` means the workbook carries no usable format metadata (no
    /// `styles.xml`, or unreadable workbook/relationships/worksheet parts)
    /// and the caller renders raw values. Degradable failures are logged and
    /// swallowed; resource-limit errors propagate per the crate's limits
    /// policy.
    pub fn from_ooxml(bytes: &[u8]) -> Result<Option<Self>, ConvertError> {
        let mut pkg = Package::open(bytes)?;
        let styles = match pkg.part("xl/styles.xml")? {
            Some(s) => s,
            None => return Ok(None),
        };
        let Some(style_codes) = parse_styles(&styles) else {
            log::warn!("spreadsheet: unreadable styles.xml; rendering cells without number formats");
            return Ok(None);
        };

        let Some(parts) = sheet_parts(&mut pkg)? else {
            log::warn!(
                "spreadsheet: unreadable workbook/relationships; rendering cells without number formats"
            );
            return Ok(Some(CellFormats { by_sheet: HashMap::new() }));
        };
        let mut by_sheet = HashMap::new();
        for (name, path) in parts {
            let Some(part) = pkg.part(&path)? else {
                log::warn!("spreadsheet: sheet part {path} missing; skipping its formats");
                continue;
            };
            if let Some(cells) = parse_sheet_formats(&part, &style_codes) {
                if !cells.is_empty() {
                    by_sheet.insert(name, cells);
                }
            }
        }
        Ok(Some(CellFormats { by_sheet }))
    }

    /// The format code for a cell, or `None` when it has none.
    pub fn code(&self, sheet: &str, row: u32, col: u32) -> Option<&str> {
        self.by_sheet.get(sheet).and_then(|cells| cells.get(&(row, col))).map(String::as_str)
    }
}

/// `style_index -> format code` from `styles.xml`, or `None` when the part
/// is structurally unusable.
fn parse_styles(bytes: &Rc<[u8]>) -> Option<Vec<Option<String>>> {
    let root = parse_xml(bytes).ok()?;
    let mut custom: HashMap<u32, String> = HashMap::new();
    for num_fmt in root.descendants(X, "numFmt") {
        let (Some(id), Some(code)) =
            (num_fmt.attr_unqualified("numFmtId"), num_fmt.attr_unqualified("formatCode"))
        else {
            continue;
        };
        if let Ok(id) = id.parse() {
            custom.insert(id, code.to_owned());
        }
    }
    let mut xfs = Vec::new();
    for xf in root.descendants(X, "xf") {
        let id: u32 = xf.attr_unqualified("numFmtId").and_then(|v| v.parse().ok()).unwrap_or(0);
        // `applyNumberFormat="0"` means the declared numFmtId is not applied;
        // the cell shows raw.
        let applied = xf
            .attr_unqualified("applyNumberFormat")
            .map(|v| v != "0" && v != "false")
            .unwrap_or(true);
        let code = if applied { super::numfmt::code_for_style(id, &custom) } else { None };
        xfs.push(code);
    }
    // A workbook whose styles part carries no style table has no format
    // metadata to apply (this also rejects leniently-repaired garbage).
    if xfs.is_empty() {
        return None;
    }
    Some(xfs)
}

/// `sheet name -> part path` from workbook.xml and its relationships, or
/// `None` when the parts are unusable.
fn sheet_parts(pkg: &mut Package<'_>) -> Result<Option<Vec<(String, String)>>, ConvertError> {
    let Some(wb) = pkg.part("xl/workbook.xml")? else {
        return Ok(None);
    };
    let Ok(root) = parse_xml(&wb) else {
        return Ok(None);
    };
    let rels = read_rels(pkg, &rels_part_for("xl/workbook.xml"))?;
    let mut parts = Vec::new();
    for sheet in root.descendants(X, "sheet") {
        let (Some(name), Some(rid)) =
            (sheet.attr_unqualified("name"), sheet.attr_qualified(R, "id"))
        else {
            continue;
        };
        let Some(target) = rels.internal_target(rid) else {
            continue;
        };
        // Relationship targets are relative to the part's directory.
        let Ok(target) = crate::package::path::resolve("xl/workbook.xml", target) else {
            continue;
        };
        parts.push((name.to_owned(), target.path.to_owned()));
    }
    Ok(Some(parts))
}

/// `(row, col) -> format code` from one worksheet part.
///
/// Cells carry a style index in `s` (absent means style 0) and a reference
/// in `r` (absent means positional within the row, one column past the
/// previous cell, per OOXML). Style indices outside the style table map to
/// no format.
fn parse_sheet_formats(
    bytes: &Rc<[u8]>,
    style_codes: &[Option<String>],
) -> Option<HashMap<(u32, u32), String>> {
    let root = parse_xml(bytes).ok()?;
    let mut out = HashMap::new();
    // Rows are 1-based in the XML and 0-based in the caller's coordinates.
    let mut next_row = 0u32;
    for row in root.descendants(X, "row") {
        let row_idx = match row.attr_unqualified("r").and_then(|v| v.parse::<u32>().ok()) {
            Some(r) => {
                let idx = r.saturating_sub(1);
                next_row = idx + 1;
                idx
            }
            None => {
                let idx = next_row;
                next_row += 1;
                idx
            }
        };
        let mut last_col = 0u32;
        for cell in row.child_elems().filter(|e| e.is(X, "c")) {
            let col = match cell.attr_unqualified("r").and_then(|r| parse_cell_ref(r).map(|(_, c)| c)) {
                Some(c) => {
                    last_col = c + 1;
                    c
                }
                None => {
                    let c = last_col;
                    last_col += 1;
                    c
                }
            };
            let style: u32 = cell.attr_unqualified("s").and_then(|v| v.parse().ok()).unwrap_or(0);
            if let Some(Some(code)) = style_codes.get(style as usize) {
                out.insert((row_idx, col), code.clone());
            }
        }
    }
    Some(out)
}

/// Parse an absolute cell reference like `A1` or `$B$12` into 0-based
/// `(row, col)`. The row is validated so garbage references do not alias
/// positions.
fn parse_cell_ref(ref_: &str) -> Option<(u32, u32)> {
    let mut col = 0u32;
    let mut digits = String::new();
    let mut seen_digit = false;
    for ch in ref_.chars() {
        match ch {
            '$' => {}
            'A'..='Z' | 'a'..='z' if !seen_digit => {
                col = col * 26 + (ch.to_ascii_uppercase() as u32 - 'A' as u32 + 1);
            }
            '0'..='9' => {
                seen_digit = true;
                digits.push(ch);
            }
            _ => return None,
        }
    }
    let row: u32 = digits.parse().ok()?;
    if col == 0 || row == 0 {
        return None;
    }
    Some((row - 1, col - 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Minimal xlsx package with the given styles XML and sheet XML,
    /// assembled with deterministic parts (pattern: `xlsx_with_merge` in
    /// `sheet/mod.rs`).
    fn xlsx_with(styles: &str, sheet: &str) -> Vec<u8> {
        let parts: &[(&str, &str)] = &[
            (
                "[Content_Types].xml",
                r#"<?xml version="1.0"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#,
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
            ("xl/styles.xml", styles),
            ("xl/worksheets/sheet1.xml", sheet),
        ];
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        for (part, body) in parts {
            w.start_file(*part, zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn style_sheet() -> &'static str {
        r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="0.00%"/></numFmts><cellXfs count="3"><xf numFmtId="0"/><xf numFmtId="164" applyNumberFormat="1"/><xf numFmtId="164" applyNumberFormat="0"/></cellXfs></styleSheet>"#
    }

    fn sheet_a1_s1() -> &'static str {
        r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="1"><v>0.65</v></c></row></sheetData></worksheet>"#
    }

    #[test]
    fn reads_custom_format_by_style_index() {
        let bytes = xlsx_with(style_sheet(), sheet_a1_s1());
        let cf = CellFormats::from_ooxml(&bytes).unwrap().unwrap();
        assert_eq!(cf.code("S", 0, 0), Some("0.00%"));
    }

    #[test]
    fn builtin_id_resolves_without_custom_entry() {
        let styles = r#"<?xml version="1.0"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><cellXfs count="2"><xf numFmtId="0"/><xf numFmtId="9"/></cellXfs></styleSheet>"#;
        let bytes = xlsx_with(styles, sheet_a1_s1());
        let cf = CellFormats::from_ooxml(&bytes).unwrap().unwrap();
        assert_eq!(cf.code("S", 0, 0), Some("0%"));
    }

    #[test]
    fn apply_number_format_zero_treats_style_as_general() {
        let bytes = xlsx_with(
            style_sheet(),
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="2"><v>0.65</v></c></row></sheetData></worksheet>"#,
        );
        let cf = CellFormats::from_ooxml(&bytes).unwrap().unwrap();
        assert_eq!(cf.code("S", 0, 0), None);
    }

    #[test]
    fn cells_without_style_attr_map_to_style_zero() {
        let bytes = xlsx_with(
            style_sheet(),
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1"><v>0.65</v></c></row></sheetData></worksheet>"#,
        );
        let cf = CellFormats::from_ooxml(&bytes).unwrap().unwrap();
        assert_eq!(cf.code("S", 0, 0), None);
    }

    #[test]
    fn cells_without_r_resolve_positionally() {
        let bytes = xlsx_with(
            style_sheet(),
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c s="1"><v>0.65</v></c><c><v>2</v></c></row></sheetData></worksheet>"#,
        );
        let cf = CellFormats::from_ooxml(&bytes).unwrap().unwrap();
        assert_eq!(cf.code("S", 0, 0), Some("0.00%"));
        assert_eq!(cf.code("S", 0, 1), None);
    }

    #[test]
    fn no_styles_part_returns_none() {
        // The styles part is empty (unusable) -> Ok(None).
        let bytes = xlsx_with("", sheet_a1_s1());
        let cf = CellFormats::from_ooxml(&bytes).unwrap();
        assert!(cf.is_none());
    }

    #[test]
    fn malformed_styles_degrades_to_none() {
        let bytes = xlsx_with("not xml at all {{{", sheet_a1_s1());
        let cf = CellFormats::from_ooxml(&bytes).unwrap();
        assert!(cf.is_none());
    }

    #[test]
    fn style_index_out_of_range_degrades() {
        let bytes = xlsx_with(
            style_sheet(),
            r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" s="99"><v>0.65</v></c></row></sheetData></worksheet>"#,
        );
        let cf = CellFormats::from_ooxml(&bytes).unwrap().unwrap();
        assert_eq!(cf.code("S", 0, 0), None);
    }

    #[test]
    fn missing_sheet_name_has_no_formats() {
        let bytes = xlsx_with(style_sheet(), sheet_a1_s1());
        let cf = CellFormats::from_ooxml(&bytes).unwrap().unwrap();
        assert_eq!(cf.code("Nope", 0, 0), None);
    }

    #[test]
    fn cell_ref_parsing() {
        assert_eq!(parse_cell_ref("A1"), Some((0, 0)));
        assert_eq!(parse_cell_ref("Z9"), Some((8, 25)));
        assert_eq!(parse_cell_ref("AA11"), Some((10, 26)));
        assert_eq!(parse_cell_ref("$B$12"), Some((11, 1)));
        assert_eq!(parse_cell_ref(""), None);
        assert_eq!(parse_cell_ref("1A"), None);
    }
}
