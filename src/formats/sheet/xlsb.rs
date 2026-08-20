//! In-house binary SpreadsheetML reader (.xlsb): the same OPC package as
//! xlsx with binary record streams (MS-XLSB) in place of XML parts. Cell
//! values, number formats, visibility and merges are decoded here; grid
//! materialization, format resolution and date rendering are shared with
//! the xlsx reader so both containers convert identically.

use super::xlsx::{
    CellFormat, MAX_COLS, MAX_ROWS, SHARED_STRINGS_REL, SheetContent, build_table, format_as_text,
    render_number, resolve_format, sibling_part_name,
};
use super::{error_literal, rk_number};
use crate::error::ConvertError;
use crate::model::{Block, Document, Inline};
use crate::package::limits;
use crate::package::relationships::{read_rels, rel_type, rels_part_for};
use crate::package::{Package, path};
use crate::shared::text::clean_text;
use std::collections::HashMap;

// MS-XLSB record type values (section 2.3, "By Number").
const BRT_ROW_HDR: u16 = 0;
const BRT_CELL_RK: u16 = 2;
const BRT_CELL_ERROR: u16 = 3;
const BRT_CELL_BOOL: u16 = 4;
const BRT_CELL_REAL: u16 = 5;
const BRT_CELL_ST: u16 = 6;
const BRT_CELL_ISST: u16 = 7;
const BRT_FMLA_STRING: u16 = 8;
const BRT_FMLA_NUM: u16 = 9;
const BRT_FMLA_BOOL: u16 = 10;
const BRT_FMLA_ERROR: u16 = 11;
const BRT_SST_ITEM: u16 = 19;
const BRT_FMT: u16 = 44;
const BRT_XF: u16 = 47;
const BRT_COL_INFO: u16 = 60;
const BRT_CELL_RSTRING: u16 = 62;
const BRT_WB_PROP: u16 = 153;
const BRT_BUNDLE_SH: u16 = 156;
const BRT_MERGE_CELL: u16 = 176;
const BRT_BEGIN_CELL_XFS: u16 = 617;
const BRT_END_CELL_XFS: u16 = 618;

pub(super) fn parse(pkg: &mut Package, wb_part: &str) -> Result<Document, ConvertError> {
    let workbook = pkg.required_part(wb_part)?;
    let (date1904, bundles) = read_workbook(&workbook)?;
    let wb_rels = read_rels(pkg, &rels_part_for(wb_part))?;

    let shared = read_optional(
        pkg,
        &sibling_part_name(&wb_rels, wb_part, SHARED_STRINGS_REL, "sharedStrings.bin"),
        read_shared_strings,
    )?;
    let xfs = read_optional(
        pkg,
        &sibling_part_name(&wb_rels, wb_part, rel_type::STYLES, "styles.bin"),
        read_styles,
    )?;

    // Visible sheets in workbook order, resolved to their parts exactly as
    // in xlsx: by relationship id, never by conventional part name.
    let mut sheets: Vec<(String, String)> = Vec::new();
    for (name, rid) in bundles {
        let Some(target) = wb_rels.internal_target(&rid) else {
            log::warn!("skipping sheet {name:?} with no worksheet relationship");
            continue;
        };
        match path::resolve(wb_part, target) {
            Ok(t) => sheets.push((name, t.path)),
            Err(e) => log::warn!("skipping sheet {name:?} with unresolvable target: {e}"),
        }
    }

    let multi_sheet = sheets.len() > 1;
    let mut doc = Document::default();
    let mut failed = 0usize;
    // One budget for the workbook, so sheets cannot multiply the cap.
    let mut slots = 0u64;
    for (name, part) in &sheets {
        let content =
            pkg.optional_part(part)?.map(|bytes| read_sheet(&bytes, &shared, &xfs, date1904));
        let content = match content {
            Some(Ok(c)) => c,
            Some(Err(e)) if e.is_fatal() => return Err(e),
            Some(Err(e)) => {
                log::warn!("skipping unreadable sheet {name:?}: {e}");
                failed += 1;
                continue;
            }
            None => {
                log::warn!("skipping unreadable sheet {name:?}");
                failed += 1;
                continue;
            }
        };
        let Some(table) = build_table(content, &mut slots)? else {
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

/// Read an optional binary part under the unified recovery policy: absent or
/// corrupt yields the default with a log; fatal resource-limit errors always
/// propagate.
fn read_optional<T: Default>(
    pkg: &mut Package,
    part: &str,
    read: fn(&[u8]) -> Result<T, ConvertError>,
) -> Result<T, ConvertError> {
    let Some(bytes) = pkg.optional_part(part)? else {
        return Ok(T::default());
    };
    match read(&bytes) {
        Ok(v) => Ok(v),
        Err(e) if e.is_fatal() => Err(e),
        Err(e) => {
            log::warn!("skipping corrupt part {part}: {e}");
            Ok(T::default())
        }
    }
}

/// `xl/workbook.bin`: the 1904 date flag from BrtWbProp, and each visible
/// sheet's name and relationship id from its BrtBundleSh.
fn read_workbook(data: &[u8]) -> Result<(bool, Vec<(String, String)>), ConvertError> {
    let mut date1904 = false;
    let mut sheets = Vec::new();
    let mut records = Records::new(data);
    while let Some((id, payload)) = records.next()? {
        match id {
            BRT_WB_PROP => date1904 = Fields::new(payload).u32()? & 1 != 0,
            BRT_BUNDLE_SH => {
                let mut f = Fields::new(payload);
                let state = f.u32()?;
                f.u32()?; // iTabID
                let rid = f.nullable_wide_string()?;
                let name = clean_text(&f.wide_string()?);
                // hsState 1 is hidden, 2 is veryHidden: omitted entirely,
                // heading included, exactly as in xlsx.
                if state == 1 || state == 2 {
                    continue;
                }
                match rid {
                    Some(rid) => sheets.push((name, rid)),
                    None => log::warn!("skipping sheet {name:?} with no worksheet relationship"),
                }
            }
            _ => {}
        }
    }
    Ok((date1904, sheets))
}

/// `styles.bin` reduced to the ordered cellXfs list, each entry's format id
/// resolved to a parsed format. Only BrtXF records between BrtBeginCellXFs
/// and BrtEndCellXFs are cell XFs; the cell-style XF collection also holds
/// BrtXF records and must not shift the indices.
fn read_styles(data: &[u8]) -> Result<Vec<CellFormat>, ConvertError> {
    let mut codes: HashMap<u32, String> = HashMap::new();
    let mut fmt_ids: Vec<u32> = Vec::new();
    let mut in_cell_xfs = false;
    let mut records = Records::new(data);
    while let Some((id, payload)) = records.next()? {
        match id {
            BRT_BEGIN_CELL_XFS => in_cell_xfs = true,
            BRT_END_CELL_XFS => in_cell_xfs = false,
            BRT_FMT => {
                let mut f = Fields::new(payload);
                let ifmt = f.u16()?;
                codes.insert(u32::from(ifmt), f.wide_string()?);
            }
            BRT_XF if in_cell_xfs => {
                let mut f = Fields::new(payload);
                f.u16()?; // ixfeParent
                fmt_ids.push(u32::from(f.u16()?));
            }
            _ => {}
        }
    }
    let custom: HashMap<u32, &str> = codes.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let mut cache: HashMap<u32, CellFormat> = HashMap::new();
    Ok(fmt_ids
        .iter()
        .map(|id| cache.entry(*id).or_insert_with(|| resolve_format(*id, &custom)).clone())
        .collect())
}

/// `sharedStrings.bin`: one cleaned entry per BrtSSTItem in order. The item
/// is a RichStr; formatting runs and phonetic data trail the string and are
/// not content.
fn read_shared_strings(data: &[u8]) -> Result<Vec<String>, ConvertError> {
    let mut out = Vec::new();
    let mut records = Records::new(data);
    while let Some((id, payload)) = records.next()? {
        if id == BRT_SST_ITEM {
            let mut f = Fields::new(payload);
            f.u8()?; // fRichStr / fExtStr flags
            out.push(clean_text(&f.wide_string()?));
        }
    }
    Ok(out)
}

/// One worksheet part. Rows come from the preceding BrtRowHdr; every cell
/// record carries only its column and cellXfs index in the common cell
/// header (MS-XLSB 2.5.10).
fn read_sheet(
    data: &[u8],
    shared: &[String],
    xfs: &[CellFormat],
    date1904: bool,
) -> Result<SheetContent, ConvertError> {
    let mut out = SheetContent::default();
    let mut row: Option<u32> = None;
    let mut records = Records::new(data);
    while let Some((id, payload)) = records.next()? {
        let mut f = Fields::new(payload);
        match id {
            BRT_ROW_HDR => {
                let rw = f.u32()?;
                if rw >= MAX_ROWS {
                    row = None;
                    continue;
                }
                row = Some(rw);
                f.skip(7)?; // ixfe, miyRw, fExtraAsc/fExtraDsc byte
                if f.u8()? & 0x10 != 0 {
                    // fDyZero: the row is hidden.
                    out.hidden_rows.insert(rw);
                }
            }
            BRT_COL_INFO => {
                let first = f.u32()?;
                let last = f.u32()?;
                f.skip(8)?; // coldx, ixfe
                let hidden = f.u16()? & 1 != 0;
                if hidden && first <= last && first < MAX_COLS {
                    out.hidden_cols.push((first, last.min(MAX_COLS - 1)));
                }
            }
            BRT_MERGE_CELL => {
                let (r1, r2, c1, c2) = (f.u32()?, f.u32()?, f.u32()?, f.u32()?);
                if r1.max(r2) >= MAX_ROWS || c1.max(c2) >= MAX_COLS {
                    log::debug!("skipping out-of-bounds merge region");
                    continue;
                }
                if r1 != r2 || c1 != c2 {
                    out.merges.push((r1.min(r2), c1.min(c2), r1.max(r2), c1.max(c2)));
                }
            }
            BRT_CELL_RK | BRT_CELL_ERROR | BRT_CELL_BOOL | BRT_CELL_REAL | BRT_CELL_ST
            | BRT_CELL_ISST | BRT_CELL_RSTRING | BRT_FMLA_STRING | BRT_FMLA_NUM | BRT_FMLA_BOOL
            | BRT_FMLA_ERROR => {
                let col = f.u32()?;
                let style = f.u32()? & 0x00FF_FFFF;
                let Some(r) = row else {
                    log::debug!("skipping cell record before any row header");
                    continue;
                };
                if col >= MAX_COLS {
                    continue;
                }
                let fmt = xfs.get(style as usize).unwrap_or(&CellFormat::General);
                let text = cell_text(id, &mut f, fmt, shared, date1904)?;
                if !text.is_empty() {
                    out.cells.insert((r, col), text);
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// A cell's rendered text from the record body after the common header.
/// Formula records carry their cached result first; the parsed formula
/// trails it and is never read.
fn cell_text(
    id: u16,
    f: &mut Fields,
    fmt: &CellFormat,
    shared: &[String],
    date1904: bool,
) -> Result<String, ConvertError> {
    Ok(match id {
        BRT_CELL_RK => render_number(fmt, rk_number(f.u32()?), date1904),
        BRT_CELL_REAL | BRT_FMLA_NUM => render_number(fmt, f.f64()?, date1904),
        BRT_CELL_ST | BRT_FMLA_STRING => format_as_text(fmt, &clean_text(&f.wide_string()?)),
        BRT_CELL_RSTRING => {
            f.u8()?; // RichStr flags; runs and phonetic data trail the string
            format_as_text(fmt, &clean_text(&f.wide_string()?))
        }
        BRT_CELL_ISST => {
            let isst = f.u32()?;
            match usize::try_from(isst).ok().and_then(|i| shared.get(i)) {
                Some(text) => format_as_text(fmt, text),
                None => {
                    log::debug!("shared string index {isst} out of range");
                    String::new()
                }
            }
        }
        BRT_CELL_BOOL | BRT_FMLA_BOOL => match f.u8()? {
            1 => "TRUE".to_string(),
            0 => "FALSE".to_string(),
            _ => String::new(),
        },
        BRT_CELL_ERROR | BRT_FMLA_ERROR => {
            let code = f.u8()?;
            match error_literal(code) {
                Some(text) => text.to_string(),
                None => {
                    log::debug!("unknown error cell code {code:#04x}");
                    String::new()
                }
            }
        }
        _ => String::new(),
    })
}

/// Iterator over one part's record stream: a 1-2 byte record type and a 1-4
/// byte payload size, both 7 bits per byte with the high bit as the
/// continuation flag (MS-XLSB 2.1.4).
struct Records<'a> {
    data: &'a [u8],
    pos: usize,
    seen: u64,
}

impl<'a> Records<'a> {
    fn new(data: &'a [u8]) -> Self {
        Records { data, pos: 0, seen: 0 }
    }

    fn next(&mut self) -> Result<Option<(u16, &'a [u8])>, ConvertError> {
        if self.pos >= self.data.len() {
            return Ok(None);
        }
        self.seen += 1;
        if self.seen > limits::MAX_RECORDS {
            return Err(ConvertError::ResourceLimit {
                limit: "max_records",
                detail: "record stream exceeds the record budget".into(),
            });
        }
        let b0 = self.byte()?;
        let id = if b0 & 0x80 != 0 {
            u16::from(b0 & 0x7F) | (u16::from(self.byte()? & 0x7F) << 7)
        } else {
            u16::from(b0)
        };
        let mut size: usize = 0;
        for i in 0..4 {
            let b = self.byte()?;
            size |= usize::from(b & 0x7F) << (7 * i);
            if b & 0x80 == 0 {
                break;
            }
            if i == 3 {
                return Err(ConvertError::malformed("record size runs past four bytes"));
            }
        }
        let end = self
            .pos
            .checked_add(size)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| ConvertError::malformed("record payload runs past the part"))?;
        let payload = &self.data[self.pos..end];
        self.pos = end;
        Ok(Some((id, payload)))
    }

    fn byte(&mut self) -> Result<u8, ConvertError> {
        let b = *self
            .data
            .get(self.pos)
            .ok_or_else(|| ConvertError::malformed("truncated record header"))?;
        self.pos += 1;
        Ok(b)
    }
}

/// Bounds-checked field reads within one record payload. Every length here
/// is attacker-controlled, so nothing is read without a check.
struct Fields<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Fields<'a> {
    fn new(data: &'a [u8]) -> Self {
        Fields { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], ConvertError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&end| end <= self.data.len())
            .ok_or_else(|| ConvertError::malformed("truncated record payload"))?;
        let bytes = &self.data[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn skip(&mut self, n: usize) -> Result<(), ConvertError> {
        self.take(n).map(|_| ())
    }

    fn u8(&mut self) -> Result<u8, ConvertError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, ConvertError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().expect("2-byte slice")))
    }

    fn u32(&mut self) -> Result<u32, ConvertError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4-byte slice")))
    }

    fn f64(&mut self) -> Result<f64, ConvertError> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().expect("8-byte slice")))
    }

    /// XLWideString (MS-XLSB 2.5.169): a u32 character count, then that many
    /// UTF-16LE code units. Unpaired surrogates decode to U+FFFD.
    fn wide_string(&mut self) -> Result<String, ConvertError> {
        let cch = self.u32()? as usize;
        let bytes = cch
            .checked_mul(2)
            .ok_or_else(|| ConvertError::malformed("oversized string length"))
            .and_then(|n| self.take(n))?;
        let units = bytes.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
        Ok(char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect())
    }

    /// XLNullableWideString: the reserved count 0xFFFFFFFF is the null
    /// string.
    fn nullable_wide_string(&mut self) -> Result<Option<String>, ConvertError> {
        if self.data.get(self.pos..self.pos + 4) == Some(&[0xFF; 4]) {
            self.pos += 4;
            return Ok(None);
        }
        self.wide_string().map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CellSlot, Table, inlines_to_plain_text};
    use std::io::Write;

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

    /// One record in canonical (shortest) framing.
    fn rec(id: u16, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        if id < 0x80 {
            out.push(id as u8);
        } else {
            out.push((id & 0x7F) as u8 | 0x80);
            out.push((id >> 7) as u8);
        }
        let mut size = payload.len() as u32;
        loop {
            let low = (size & 0x7F) as u8;
            size >>= 7;
            if size == 0 {
                out.push(low);
                break;
            }
            out.push(low | 0x80);
        }
        out.extend_from_slice(payload);
        out
    }

    fn wstr(s: &str) -> Vec<u8> {
        let units: Vec<u16> = s.encode_utf16().collect();
        let mut out = (units.len() as u32).to_le_bytes().to_vec();
        for u in units {
            out.extend_from_slice(&u.to_le_bytes());
        }
        out
    }

    /// The 8-byte common cell header.
    fn cell(col: u32, style: u32) -> Vec<u8> {
        [col.to_le_bytes(), style.to_le_bytes()].concat()
    }

    fn row_hdr(r: u32, hidden: bool) -> Vec<u8> {
        let mut p = r.to_le_bytes().to_vec();
        p.extend_from_slice(&[0; 4]); // ixfe
        p.extend_from_slice(&[0; 2]); // miyRw
        p.push(0); // fExtraAsc / fExtraDsc
        p.push(if hidden { 0x10 } else { 0 }); // iOutLevel..fDyZero..
        p.push(0); // fPhShow
        p.extend_from_slice(&0u32.to_le_bytes()); // ccolspan
        rec(BRT_ROW_HDR, &p)
    }

    fn real_cell(col: u32, style: u32, v: f64) -> Vec<u8> {
        let mut p = cell(col, style);
        p.extend_from_slice(&v.to_le_bytes());
        rec(BRT_CELL_REAL, &p)
    }

    fn col_info(first: u32, last: u32, hidden: bool) -> Vec<u8> {
        let mut p = first.to_le_bytes().to_vec();
        p.extend_from_slice(&last.to_le_bytes());
        p.extend_from_slice(&[0; 8]); // coldx, ixfe
        p.extend_from_slice(&u16::from(hidden).to_le_bytes());
        rec(BRT_COL_INFO, &p)
    }

    fn merge(r1: u32, r2: u32, c1: u32, c2: u32) -> Vec<u8> {
        let p = [r1.to_le_bytes(), r2.to_le_bytes(), c1.to_le_bytes(), c2.to_le_bytes()].concat();
        rec(BRT_MERGE_CELL, &p)
    }

    /// styles.bin: custom BrtFmt entries and the cellXfs list by numFmtId. A
    /// cell-style BrtXF precedes BrtBeginCellXFs to prove it shifts nothing.
    fn styles(fmts: &[(u16, &str)], xf_fmt_ids: &[u16]) -> Vec<u8> {
        let xf = |parent: u16, ifmt: u16| {
            let mut p = parent.to_le_bytes().to_vec();
            p.extend_from_slice(&ifmt.to_le_bytes());
            p.extend_from_slice(&[0; 12]);
            rec(BRT_XF, &p)
        };
        let mut out = Vec::new();
        for (id, code) in fmts {
            let mut p = id.to_le_bytes().to_vec();
            p.extend(wstr(code));
            out.extend(rec(BRT_FMT, &p));
        }
        out.extend(xf(0xFFFF, 9)); // cell-style XF outside cellXfs
        out.extend(rec(BRT_BEGIN_CELL_XFS, &(xf_fmt_ids.len() as u32).to_le_bytes()));
        for id in xf_fmt_ids {
            out.extend(xf(0, *id));
        }
        out.extend(rec(BRT_END_CELL_XFS, &[]));
        out
    }

    fn shared_strings(items: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for s in items {
            let mut p = vec![0u8]; // no rich runs, no phonetic data
            p.extend(wstr(s));
            out.extend(rec(BRT_SST_ITEM, &p));
        }
        out
    }

    /// Assemble a workbook: (name, hsState, sheet records) per sheet, plus
    /// optional styles and shared-string parts.
    #[derive(Default)]
    struct Wb<'a> {
        sheets: Vec<(&'a str, u32, Vec<u8>)>,
        styles: Option<Vec<u8>>,
        shared: Option<Vec<u8>>,
        date1904: bool,
    }

    impl Wb<'_> {
        fn build(&self) -> Vec<u8> {
            let mut workbook = Vec::new();
            let mut prop = u32::from(self.date1904).to_le_bytes().to_vec();
            prop.extend_from_slice(&0u32.to_le_bytes()); // dwThemeVersion
            prop.extend(wstr("")); // strName
            workbook.extend(rec(BRT_WB_PROP, &prop));
            let mut rels = String::new();
            for (i, (name, state, _)) in self.sheets.iter().enumerate() {
                let id = i + 1;
                let mut p = state.to_le_bytes().to_vec();
                p.extend_from_slice(&(id as u32).to_le_bytes()); // iTabID
                p.extend(wstr(&format!("rId{id}")));
                p.extend(wstr(name));
                workbook.extend(rec(BRT_BUNDLE_SH, &p));
                rels.push_str(&format!(
                    r#"<Relationship Id="rId{id}" Type="{WS_REL}" Target="worksheets/sheet{id}.bin"/>"#
                ));
            }
            if self.styles.is_some() {
                rels.push_str(&format!(
                    r#"<Relationship Id="rId90" Type="{STYLES_REL}" Target="styles.bin"/>"#
                ));
            }
            if self.shared.is_some() {
                rels.push_str(&format!(
                    r#"<Relationship Id="rId91" Type="{SHARED_STRINGS_REL}" Target="sharedStrings.bin"/>"#
                ));
            }
            let rels = format!(
                r#"<?xml version="1.0"?><Relationships xmlns="{PKG_RELS}">{rels}</Relationships>"#
            );
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            let opts = zip::write::SimpleFileOptions::default();
            let mut add = |name: &str, body: &[u8]| {
                zip.start_file(name, opts).unwrap();
                zip.write_all(body).unwrap();
            };
            add("xl/workbook.bin", &workbook);
            add("xl/_rels/workbook.bin.rels", rels.as_bytes());
            for (i, (_, _, body)) in self.sheets.iter().enumerate() {
                add(&format!("xl/worksheets/sheet{}.bin", i + 1), body);
            }
            if let Some(styles) = &self.styles {
                add("xl/styles.bin", styles);
            }
            if let Some(shared) = &self.shared {
                add("xl/sharedStrings.bin", shared);
            }
            zip.finish().unwrap().into_inner()
        }
    }

    fn one_sheet(body: Vec<u8>) -> Wb<'static> {
        Wb { sheets: vec![("S", 0, body)], ..Wb::default() }
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

    #[test]
    fn record_framing_uses_seven_bit_groups() {
        // A record id over 0x7F takes two bytes, a payload size over 0x7F
        // takes more than one; the exact encodings are pinned so a framing
        // bug cannot hide behind a matching writer bug.
        let mut stream = rec(BRT_BUNDLE_SH, &[7u8; 300]);
        assert_eq!(&stream[..4], &[0x9C, 0x01, 0xAC, 0x02]);
        stream.extend(rec(BRT_CELL_RK, &[1, 2, 3]));
        let mut records = Records::new(&stream);
        let (id, payload) = records.next().unwrap().unwrap();
        assert_eq!((id, payload.len()), (BRT_BUNDLE_SH, 300));
        let (id, payload) = records.next().unwrap().unwrap();
        assert_eq!((id, payload), (BRT_CELL_RK, &[1u8, 2, 3][..]));
        assert!(records.next().unwrap().is_none());
    }

    #[test]
    fn truncated_record_stream_is_malformed() {
        let mut stream = rec(BRT_CELL_RK, &[0u8; 10]);
        stream.truncate(stream.len() - 1);
        let mut records = Records::new(&stream);
        assert!(records.next().is_err(), "payload shorter than its declared size must error");
    }

    #[test]
    fn rk_values_decode_both_kinds_with_and_without_x100() {
        // Integer, negative integer (arithmetic shift), integer / 100,
        // float from the high 30 bits, float / 100.
        assert_eq!(rk_number((1 << 2) | 2), 1.0);
        assert_eq!(rk_number(((-1i32 as u32) << 2) | 2), -1.0);
        assert_eq!(rk_number((12345 << 2) | 2 | 1), 123.45);
        assert_eq!(rk_number((1.5f64.to_bits() >> 32) as u32), 1.5);
        assert_eq!(rk_number((1.5f64.to_bits() >> 32) as u32 | 1), 0.015);
    }

    #[test]
    fn number_formats_apply_to_stored_values() {
        // Same expectations as the xlsx reader: percent and currency render
        // their display value, a date format keeps the ISO rendering, and
        // the cell-style XF in styles.bin does not shift cellXfs indices.
        let mut body = row_hdr(0, false);
        body.extend({
            let mut p = cell(0, 1);
            p.extend_from_slice(&(((75 << 2) | 2 | 1) as u32).to_le_bytes()); // RK 0.75 via x100
            rec(BRT_CELL_RK, &p)
        });
        body.extend(real_cell(1, 2, 1234.5));
        body.extend(real_cell(2, 3, 46096.0));
        body.extend(real_cell(3, 0, 1234.5)); // General
        let wb = Wb {
            styles: Some(styles(
                &[(164, "0.0%"), (165, "\"$\"#,##0.00"), (166, "mm/dd/yyyy")],
                &[0, 164, 165, 166],
            )),
            ..one_sheet(body)
        };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(
            texts(first_table(&doc)),
            vec![vec!["75.0%", "$1,234.50", "2026-03-15", "1234.5"]]
        );
    }

    #[test]
    fn strings_resolve_from_the_shared_table_and_inline() {
        let mut body = row_hdr(0, false);
        body.extend({
            let mut p = cell(0, 0);
            p.extend_from_slice(&1u32.to_le_bytes());
            rec(BRT_CELL_ISST, &p)
        });
        body.extend({
            let mut p = cell(1, 0);
            p.extend(wstr("  inline  "));
            rec(BRT_CELL_ST, &p)
        });
        let wb = Wb { shared: Some(shared_strings(&["zeroth", "shared"])), ..one_sheet(body) };
        let doc = parse(&wb.build()).unwrap();
        // Untrimmed: leading/trailing whitespace in a cell is source content.
        assert_eq!(texts(first_table(&doc)), vec![vec!["shared", "  inline  "]]);
    }

    #[test]
    fn bool_and_error_cells_render_literally() {
        let mut body = row_hdr(0, false);
        let byte_cell = |id, col, v: u8| {
            let mut p = cell(col, 0);
            p.push(v);
            rec(id, &p)
        };
        body.extend(byte_cell(BRT_CELL_BOOL, 0, 1));
        body.extend(byte_cell(BRT_CELL_BOOL, 1, 0));
        body.extend(byte_cell(BRT_CELL_ERROR, 2, 0x07));
        body.extend(byte_cell(BRT_CELL_ERROR, 3, 0x2A));
        let doc = parse(&one_sheet(body).build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["TRUE", "FALSE", "#DIV/0!", "#N/A"]]);
    }

    #[test]
    fn formula_records_render_their_cached_result() {
        // The trailing GrbitFmla and CellParsedFormula bytes are payload the
        // reader must skip untouched.
        let mut body = row_hdr(0, false);
        body.extend({
            let mut p = cell(0, 0);
            p.extend_from_slice(&2.5f64.to_le_bytes());
            p.extend_from_slice(&[0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x1E, 0x05, 0x00]);
            rec(BRT_FMLA_NUM, &p)
        });
        body.extend({
            let mut p = cell(1, 0);
            p.extend(wstr("computed"));
            p.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
            rec(BRT_FMLA_STRING, &p)
        });
        let doc = parse(&one_sheet(body).build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["2.5", "computed"]]);
    }

    #[test]
    fn hidden_rows_columns_and_sheets_are_omitted() {
        let mut visible = col_info(1, 1, true);
        visible.extend(row_hdr(0, false));
        visible.extend(real_cell(0, 0, 1.0));
        visible.extend(real_cell(1, 0, 2.0)); // hidden column
        visible.extend(real_cell(2, 0, 3.0));
        visible.extend(row_hdr(1, true));
        visible.extend(real_cell(0, 0, 4.0)); // hidden row
        visible.extend(row_hdr(2, false));
        visible.extend(real_cell(0, 0, 5.0));
        let mut secret = row_hdr(0, false);
        secret.extend(real_cell(0, 0, 9.0));
        let wb = Wb {
            sheets: vec![
                ("Shown", 0, visible),
                ("Hidden", 1, secret.clone()),
                ("VeryHidden", 2, secret),
            ],
            ..Wb::default()
        };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(doc.blocks.len(), 1, "hidden sheets must add no heading and no table");
        assert_eq!(texts(first_table(&doc)), vec![vec!["1", "3"], vec!["5", ""]]);
    }

    #[test]
    fn merge_extends_past_the_populated_range() {
        // Issue #8, matching the xlsx reader: the only populated cell
        // anchors F1:O3, so the grid widens to the full 3x10 extent.
        let mut body = row_hdr(0, false);
        body.extend({
            let mut p = cell(5, 0);
            p.extend(wstr("wide"));
            rec(BRT_CELL_ST, &p)
        });
        body.extend(merge(0, 2, 5, 14));
        let doc = parse(&one_sheet(body).build()).unwrap();
        let table = first_table(&doc);
        assert_eq!((table.grid.len(), table.grid[1].len()), (3, 10));
        let CellSlot::Origin(cell) = &table.grid[0][0] else {
            panic!("expected the merge origin at (0,0)");
        };
        assert_eq!((cell.col_span, cell.row_span), (10, 3));
    }

    #[test]
    fn date1904_serials_shift_epoch() {
        let mut body = row_hdr(0, false);
        body.extend(real_cell(0, 0, 100.0));
        let wb = Wb {
            styles: Some(styles(&[(164, "yyyy-mm-dd")], &[164])),
            date1904: true,
            ..one_sheet(body)
        };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["1904-04-10"]]);
    }

    #[test]
    fn unknown_records_are_skipped_without_desync() {
        // A record the reader does not know (BrtWsDim, 148) must be skipped
        // by its declared size, leaving the following cells intact.
        let mut body = rec(148, &[0u8; 16]);
        body.extend(row_hdr(0, false));
        body.extend(real_cell(0, 0, 7.0));
        let doc = parse(&one_sheet(body).build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["7"]]);
    }
}
