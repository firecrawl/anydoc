//! In-house legacy Excel reader (.xls): OLE2 compound file holding a BIFF
//! record stream ([MS-XLS]). BIFF8 is the target; BIFF5/BIFF7 streams
//! degrade to their byte-string record layouts instead of erroring. Cell
//! values resolve their number format through the same engine and grid
//! assembly as the SpreadsheetML reader, so a workbook saved as .xls and as
//! .xlsx converts identically.

use super::xlsx::{
    CellFormat, SheetContent, build_table, format_as_text, render_number, resolve_format,
};
use super::{error_literal, rk_number};
use crate::error::ConvertError;
use crate::model::{Block, Document, Inline};
use crate::package::limits;
use crate::shared::binary::{get_u16, get_u32, read_ole_stream};
use crate::shared::text::clean_text;
use std::collections::HashMap;
use std::io::Cursor;

// Record types ([MS-XLS] 2.3.2), BIFF5-BIFF8 numbering.
const BOF: u16 = 0x0809;
const EOF_REC: u16 = 0x000A;
const FILEPASS: u16 = 0x002F;
const CODEPAGE: u16 = 0x0042;
const DATEMODE: u16 = 0x0022;
const BOUNDSHEET: u16 = 0x0085;
const SST: u16 = 0x00FC;
const CONTINUE: u16 = 0x003C;
const FORMAT: u16 = 0x041E;
const XF: u16 = 0x00E0;
const ROW: u16 = 0x0208;
const COLINFO: u16 = 0x007D;
const MERGEDCELLS: u16 = 0x00E5;
const LABELSST: u16 = 0x00FD;
const LABEL: u16 = 0x0204;
const RSTRING: u16 = 0x00D6;
const NUMBER: u16 = 0x0203;
const RK: u16 = 0x027E;
const MULRK: u16 = 0x00BD;
const BOOLERR: u16 = 0x0205;
const FORMULA: u16 = 0x0006;
const STRING: u16 = 0x0207;

/// BOF `dt` value for a worksheet (or dialog sheet) substream.
const WORKSHEET_SUBSTREAM: u16 = 0x0010;

/// The BIFF8 grid is 256 columns; a larger column index is not a real cell.
const MAX_COLS: u32 = 256;

pub(super) fn parse(bytes: &[u8]) -> Result<Document, ConvertError> {
    let mut ole = cfb::CompoundFile::open(Cursor::new(bytes))
        .map_err(|e| ConvertError::malformed(format!("not an OLE2 compound file: {e}")))?;
    let data = workbook_stream(&mut ole)?;
    let mut records = 0u64;
    let globals = read_globals(&data, &mut records)?;

    let visible: Vec<&BoundSheet> = globals.sheets.iter().filter(|s| s.visible).collect();
    let multi_sheet = visible.len() > 1;
    let mut doc = Document::default();
    let mut failed = 0usize;
    // One budget for the workbook, so sheets cannot multiply the cap.
    let mut slots = 0u64;
    for sheet in &visible {
        let Some(content) = read_sheet(&data, &globals, sheet.offset, &mut records)? else {
            log::warn!("skipping unreadable sheet {:?}", sheet.name);
            failed += 1;
            continue;
        };
        let Some(table) = build_table(content, &mut slots)? else {
            continue;
        };
        if multi_sheet {
            doc.blocks.push(Block::heading(2, vec![Inline::plain(sheet.name.clone())]));
        }
        doc.blocks.push(Block::Table(table));
    }
    if !visible.is_empty() && failed == visible.len() {
        return Err(ConvertError::malformed("no sheet in the workbook could be read"));
    }
    Ok(doc)
}

/// The BIFF stream: `Workbook` (BIFF8) or `Book` (older BIFF), matched
/// case-insensitively like detection because producers vary.
fn workbook_stream<R: std::io::Read + std::io::Seek>(
    ole: &mut cfb::CompoundFile<R>,
) -> Result<Vec<u8>, ConvertError> {
    let name = ole
        .read_root_storage()
        .find(|e| {
            e.is_stream()
                && (e.name().eq_ignore_ascii_case("Workbook")
                    || e.name().eq_ignore_ascii_case("Book"))
        })
        .map(|e| e.name().to_string())
        .ok_or(ConvertError::MissingPart { part: "Workbook".to_string() })?;
    read_ole_stream(ole, &name)
}

/// The record at `pos`: type, payload, and the position after it. `None` on
/// a truncated header or payload.
fn record_at(data: &[u8], pos: usize) -> Option<(u16, &[u8], usize)> {
    let rec_type = get_u16(data, pos)?;
    let len = get_u16(data, pos.checked_add(2)?)? as usize;
    let body_at = pos.checked_add(4)?;
    let body = data.get(body_at..body_at.checked_add(len)?)?;
    Some((rec_type, body, body_at + len))
}

/// A record: type, payload, position after it.
type Record<'a> = (u16, &'a [u8], usize);

/// `record_at` charging the stream-wide record budget.
fn next_record<'a>(
    data: &'a [u8],
    pos: usize,
    records: &mut u64,
) -> Result<Option<Record<'a>>, ConvertError> {
    let Some(rec) = record_at(data, pos) else {
        // Trailing bytes too short to form a record: the stream was cut, so
        // whatever was read is partial rather than complete.
        if pos < data.len() {
            log::warn!("workbook stream ends mid-record at byte {pos}; output may be partial");
        }
        return Ok(None);
    };
    *records += 1;
    if *records > limits::MAX_RECORDS {
        return Err(ConvertError::ResourceLimit {
            limit: "max_records",
            detail: format!("workbook stream exceeds {} records", limits::MAX_RECORDS),
        });
    }
    Ok(Some(rec))
}

/// The record body at `pos - body.len() - 4` plus the bodies of the
/// CONTINUE records that immediately follow it, and the position after them.
fn continued<'a>(
    data: &'a [u8],
    body: &'a [u8],
    mut pos: usize,
    records: &mut u64,
) -> Result<(Vec<&'a [u8]>, usize), ConvertError> {
    let mut segs = vec![body];
    while let Some((rec_type, cont, next)) = next_record(data, pos, records)? {
        if rec_type != CONTINUE {
            *records -= 1;
            break;
        }
        segs.push(cont);
        pos = next;
    }
    Ok((segs, pos))
}

/// Reader over a record's segments (base body plus CONTINUE bodies). Fixed
/// fields never straddle a segment boundary, but may start exactly on one;
/// only string character data crosses boundaries, and the string readers
/// handle the repeated option-flags byte themselves.
struct SegReader<'a> {
    segs: Vec<&'a [u8]>,
    seg: usize,
    off: usize,
}

impl<'a> SegReader<'a> {
    fn new(segs: Vec<&'a [u8]>) -> SegReader<'a> {
        SegReader { segs, seg: 0, off: 0 }
    }

    /// Bytes left in the current segment.
    fn in_seg(&self) -> usize {
        self.segs.get(self.seg).map_or(0, |s| s.len() - self.off)
    }

    /// Hop over exhausted segments so a field can start at a boundary.
    fn normalize(&mut self) {
        while self.seg < self.segs.len() && self.in_seg() == 0 {
            self.seg += 1;
            self.off = 0;
        }
    }

    /// Move to the start of the next segment; `None` when there is none.
    fn next_seg(&mut self) -> Option<()> {
        if self.seg + 1 < self.segs.len() {
            self.seg += 1;
            self.off = 0;
            Some(())
        } else {
            None
        }
    }

    /// `n` bytes from within one segment.
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        self.normalize();
        let seg = self.segs.get(self.seg)?;
        let out = seg.get(self.off..self.off.checked_add(n)?)?;
        self.off += n;
        Some(out)
    }

    fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.bytes(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        self.bytes(4).and_then(|b| Some(u32::from_le_bytes(b.try_into().ok()?)))
    }

    /// Skip `n` bytes across segment boundaries (non-character data carries
    /// no repeated flags byte).
    fn skip(&mut self, mut n: usize) -> Option<()> {
        while n > 0 {
            self.normalize();
            let step = self.in_seg().min(n);
            if step == 0 {
                return None;
            }
            self.off += step;
            n -= step;
        }
        Some(())
    }
}

/// A BIFF8 Unicode string: XLUnicodeString, ShortXLUnicodeString (`short`),
/// or XLUnicodeRichExtendedString (`rich`). Character data may continue
/// into following segments; at every such boundary the option-flags byte is
/// repeated and the encoding can switch between 8-bit compressed and 16-bit
/// UTF-16, so it is re-read rather than carried over. Rich runs and
/// phonetic data are skipped, matching the xlsx reader's handling of `rPh`.
fn read_biff8_string(r: &mut SegReader, short: bool, rich: bool) -> Option<String> {
    let cch = if short { r.u8()? as usize } else { r.u16()? as usize };
    let flags = r.u8()?;
    let mut wide = flags & 0x01 != 0;
    let runs = if rich && flags & 0x08 != 0 { r.u16()? as usize } else { 0 };
    let ext = if rich && flags & 0x04 != 0 { r.u32()? as usize } else { 0 };
    let mut units: Vec<u16> = Vec::new();
    let mut remaining = cch;
    while remaining > 0 {
        if r.in_seg() == 0 {
            r.next_seg()?;
            wide = r.u8()? & 0x01 != 0;
        }
        let unit = if wide { 2 } else { 1 };
        let take = (r.in_seg() / unit).min(remaining);
        if take == 0 {
            // A dangling half character: outside the format.
            return None;
        }
        let bytes = r.bytes(take * unit)?;
        if wide {
            units.extend(bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])));
        } else {
            units.extend(bytes.iter().map(|&b| u16::from(b)));
        }
        remaining -= take;
    }
    // A truncated trailer loses only the strings after this one; the reader
    // then runs dry and the caller stops.
    let _ = r.skip(runs * 4).and_then(|()| r.skip(ext));
    Some(String::from_utf16_lossy(&units))
}

/// A BIFF5/BIFF7 byte string (no flags byte), decoded per the workbook's
/// CODEPAGE record.
fn read_byte_string(
    r: &mut SegReader,
    short: bool,
    encoding: &'static encoding_rs::Encoding,
) -> Option<String> {
    let cch = if short { r.u8()? as usize } else { r.u16()? as usize };
    let mut bytes = Vec::with_capacity(cch);
    let mut remaining = cch;
    while remaining > 0 {
        r.normalize();
        let take = r.in_seg().min(remaining);
        if take == 0 {
            return None;
        }
        bytes.extend_from_slice(r.bytes(take)?);
        remaining -= take;
    }
    let (text, _) = encoding.decode_without_bom_handling(&bytes);
    Some(text.into_owned())
}

/// ANSI code page from the CODEPAGE record, for BIFF5 byte strings (BIFF8
/// strings carry their own encoding flag).
fn codepage_encoding(cp: u16) -> &'static encoding_rs::Encoding {
    use encoding_rs::*;
    match cp {
        874 => WINDOWS_874,
        932 => SHIFT_JIS,
        936 => GBK,
        949 => EUC_KR,
        950 => BIG5,
        1250 => WINDOWS_1250,
        1251 => WINDOWS_1251,
        1253 => WINDOWS_1253,
        1254 => WINDOWS_1254,
        1255 => WINDOWS_1255,
        1256 => WINDOWS_1256,
        1257 => WINDOWS_1257,
        1258 => WINDOWS_1258,
        _ => WINDOWS_1252,
    }
}

struct BoundSheet {
    name: String,
    /// Absolute stream offset of the sheet substream's BOF record.
    offset: usize,
    visible: bool,
}

struct Globals {
    biff8: bool,
    date1904: bool,
    encoding: &'static encoding_rs::Encoding,
    sst: Vec<String>,
    /// The XF table in record order; a cell's ixfe indexes it directly.
    xfs: Vec<CellFormat>,
    sheets: Vec<BoundSheet>,
}

impl Globals {
    fn format(&self, ixfe: u16) -> &CellFormat {
        self.xfs.get(usize::from(ixfe)).unwrap_or(&CellFormat::General)
    }

    fn read_string(&self, r: &mut SegReader, short: bool) -> Option<String> {
        if self.biff8 {
            read_biff8_string(r, short, false)
        } else {
            read_byte_string(r, short, self.encoding)
        }
    }
}

/// The workbook globals substream: SST, FORMAT/XF tables, sheet directory,
/// date system, encryption marker.
fn read_globals(data: &[u8], records: &mut u64) -> Result<Globals, ConvertError> {
    let Some((rec_type, body, mut pos)) = record_at(data, 0) else {
        return Err(ConvertError::malformed("empty workbook stream"));
    };
    if rec_type != BOF {
        return Err(ConvertError::malformed("workbook stream does not start with a BOF record"));
    }
    let mut globals = Globals {
        biff8: get_u16(body, 0) == Some(0x0600),
        date1904: false,
        encoding: encoding_rs::WINDOWS_1252,
        sst: Vec::new(),
        xfs: Vec::new(),
        sheets: Vec::new(),
    };
    let mut formats: HashMap<u32, String> = HashMap::new();
    let mut xf_ifmts: Vec<u16> = Vec::new();
    let mut depth = 1usize;
    while let Some((rec_type, body, next)) = next_record(data, pos, records)? {
        pos = next;
        match rec_type {
            BOF => depth += 1,
            EOF_REC => {
                if depth == 1 {
                    break;
                }
                depth -= 1;
            }
            _ if depth > 1 => {}
            FILEPASS => return Err(ConvertError::Encrypted),
            CODEPAGE => {
                if let Some(cp) = get_u16(body, 0) {
                    globals.encoding = codepage_encoding(cp);
                }
            }
            DATEMODE => globals.date1904 = get_u16(body, 0) == Some(1),
            BOUNDSHEET => {
                if let Some(sheet) = read_boundsheet(body, &globals) {
                    globals.sheets.push(sheet);
                }
            }
            FORMAT => {
                let Some(ifmt) = get_u16(body, 0) else {
                    continue;
                };
                let mut r = SegReader::new(vec![&body[2..]]);
                // BIFF5 FORMAT carries a one-byte length, BIFF8 a two-byte.
                if let Some(code) = globals.read_string(&mut r, !globals.biff8) {
                    formats.insert(u32::from(ifmt), code);
                }
            }
            XF => {
                // ixfe is 16-bit, so the table never usefully exceeds it.
                if xf_ifmts.len() <= usize::from(u16::MAX)
                    && let Some(ifmt) = get_u16(body, 2)
                {
                    xf_ifmts.push(ifmt);
                }
            }
            SST if globals.biff8 => {
                let (segs, after) = continued(data, body, pos, records)?;
                globals.sst = read_sst(&segs);
                pos = after;
            }
            _ => {}
        }
    }
    // Resolved after the pass: FORMAT records are not ordered relative to
    // the XFs that reference them.
    let custom: HashMap<u32, &str> =
        formats.iter().map(|(&id, code)| (id, code.as_str())).collect();
    let mut cache: HashMap<u16, CellFormat> = HashMap::new();
    globals.xfs = xf_ifmts
        .iter()
        .map(|&ifmt| {
            cache.entry(ifmt).or_insert_with(|| resolve_format(u32::from(ifmt), &custom)).clone()
        })
        .collect();
    Ok(globals)
}

/// BOUNDSHEET: substream offset, hidden state, sheet type, name. Chart and
/// macro sheets stay listed (their substreams fail the worksheet check and
/// count as unreadable, matching the xlsx reader's treatment of their
/// parts); VBA modules are not sheets in any container and are dropped.
fn read_boundsheet(body: &[u8], globals: &Globals) -> Option<BoundSheet> {
    let offset = get_u32(body, 0)? as usize;
    let state = body.get(4)? & 0x03;
    if *body.get(5)? == 0x06 {
        return None;
    }
    let mut r = SegReader::new(vec![body.get(6..)?]);
    let name = globals.read_string(&mut r, true)?;
    Some(BoundSheet { name: clean_text(&name), offset, visible: state == 0 })
}

/// The shared string table (trap 1: strings routinely split across
/// CONTINUE records, mid-character-data). A malformed tail keeps the
/// entries read so far; LABELSST lookups past them log and stay empty.
fn read_sst(segs: &[&[u8]]) -> Vec<String> {
    let mut r = SegReader::new(segs.to_vec());
    let unique = match (r.u32(), r.u32()) {
        (Some(_total), Some(unique)) => unique as usize,
        _ => return Vec::new(),
    };
    let mut out = Vec::new();
    while out.len() < unique {
        let Some(text) = read_biff8_string(&mut r, false, true) else {
            if out.len() < unique {
                log::debug!("shared string table truncated at entry {}", out.len());
            }
            break;
        };
        out.push(clean_text(&text));
    }
    out
}

/// A cell record's leading (row, column, ixfe); `None` when the column is
/// outside the grid the format defines.
fn cell_ref(body: &[u8]) -> Option<(u32, u32, u16)> {
    let row = u32::from(get_u16(body, 0)?);
    let col = u32::from(get_u16(body, 2)?);
    let ixfe = get_u16(body, 4)?;
    (col < MAX_COLS).then_some((row, col, ixfe))
}

fn get_f64(body: &[u8], off: usize) -> Option<f64> {
    Some(f64::from_le_bytes(body.get(off..off.checked_add(8)?)?.try_into().ok()?))
}

/// One worksheet substream into the shared `SheetContent` shape. `Ok(None)`
/// means the substream is missing or not a worksheet (chart or macro
/// sheets), which the caller counts as unreadable.
fn read_sheet(
    data: &[u8],
    globals: &Globals,
    offset: usize,
    records: &mut u64,
) -> Result<Option<SheetContent>, ConvertError> {
    let Some((rec_type, body, mut pos)) = record_at(data, offset) else {
        return Ok(None);
    };
    if rec_type != BOF || get_u16(body, 2) != Some(WORKSHEET_SUBSTREAM) {
        return Ok(None);
    }
    let mut out = SheetContent::default();
    let mut depth = 1usize;
    // A FORMULA whose cached value is a string: (row, col, ixfe) waiting
    // for the STRING record that carries the text.
    let mut pending: Option<(u32, u32, u16)> = None;
    while let Some((rec_type, body, next)) = next_record(data, pos, records)? {
        pos = next;
        match rec_type {
            BOF => depth += 1,
            EOF_REC => {
                if depth == 1 {
                    break;
                }
                depth -= 1;
            }
            _ if depth > 1 => {}
            ROW => {
                if body.get(12).is_some_and(|flags| flags & 0x20 != 0)
                    && let Some(row) = get_u16(body, 0)
                {
                    out.hidden_rows.insert(u32::from(row));
                }
            }
            COLINFO => {
                if let (Some(first), Some(last), Some(flags)) =
                    (get_u16(body, 0), get_u16(body, 2), get_u16(body, 8))
                    && flags & 0x01 != 0
                    && first <= last
                {
                    out.hidden_cols.push((u32::from(first), u32::from(last).min(MAX_COLS - 1)));
                }
            }
            MERGEDCELLS => {
                let count = usize::from(get_u16(body, 0).unwrap_or(0))
                    .min(body.len().saturating_sub(2) / 8);
                for i in 0..count {
                    let at = 2 + i * 8;
                    let (Some(r1), Some(r2), Some(c1), Some(c2)) = (
                        get_u16(body, at),
                        get_u16(body, at + 2),
                        get_u16(body, at + 4),
                        get_u16(body, at + 6),
                    ) else {
                        break;
                    };
                    let (r1, r2) = (u32::from(r1.min(r2)), u32::from(r1.max(r2)));
                    let (c1, c2) = (u32::from(c1.min(c2)), u32::from(c1.max(c2)));
                    if c1 >= MAX_COLS || (r1 == r2 && c1 == c2) {
                        continue;
                    }
                    out.merges.push((r1, c1, r2, c2.min(MAX_COLS - 1)));
                }
            }
            LABELSST => {
                if let Some((row, col, ixfe)) = cell_ref(body)
                    && let Some(isst) = get_u32(body, 6)
                {
                    match globals.sst.get(isst as usize) {
                        Some(text) => {
                            put(&mut out, row, col, format_as_text(globals.format(ixfe), text));
                        }
                        None => log::debug!("shared string index {isst} out of range"),
                    }
                }
            }
            LABEL | RSTRING => {
                let (segs, after) = continued(data, body, pos, records)?;
                pos = after;
                if let Some((row, col, ixfe)) = cell_ref(body)
                    && let Some(mut r) = string_reader(&segs, 6)
                    && let Some(text) = globals.read_string(&mut r, false)
                {
                    // RSTRING's trailing rich runs are formatting, not text.
                    put(
                        &mut out,
                        row,
                        col,
                        format_as_text(globals.format(ixfe), &clean_text(&text)),
                    );
                }
            }
            NUMBER => {
                if let Some((row, col, ixfe)) = cell_ref(body)
                    && let Some(n) = get_f64(body, 6)
                {
                    put(
                        &mut out,
                        row,
                        col,
                        render_number(globals.format(ixfe), n, globals.date1904),
                    );
                }
            }
            RK => {
                if let Some((row, col, ixfe)) = cell_ref(body)
                    && let Some(rk) = get_u32(body, 6)
                {
                    let n = rk_number(rk);
                    put(
                        &mut out,
                        row,
                        col,
                        render_number(globals.format(ixfe), n, globals.date1904),
                    );
                }
            }
            MULRK => {
                // rw, colFirst, then 6-byte (ixfe, RK) pairs; colLast is
                // redundant with the record length.
                if let (Some(row), Some(col_first)) = (get_u16(body, 0), get_u16(body, 2)) {
                    for i in 0..body.len().saturating_sub(6) / 6 {
                        let (Some(ixfe), Some(rk)) =
                            (get_u16(body, 4 + i * 6), get_u32(body, 6 + i * 6))
                        else {
                            break;
                        };
                        let col = u32::from(col_first) + i as u32;
                        if col >= MAX_COLS {
                            break;
                        }
                        let n = rk_number(rk);
                        let text = render_number(globals.format(ixfe), n, globals.date1904);
                        put(&mut out, u32::from(row), col, text);
                    }
                }
            }
            BOOLERR => {
                if let Some((row, col, _ixfe)) = cell_ref(body)
                    && let (Some(&value), Some(&is_err)) = (body.get(6), body.get(7))
                {
                    let text = match (is_err, value) {
                        (0, 0) => Some("FALSE"),
                        (0, _) => Some("TRUE"),
                        (1, code) => error_literal(code),
                        _ => None,
                    };
                    if let Some(text) = text {
                        put(&mut out, row, col, text.to_string());
                    }
                }
            }
            FORMULA => {
                if let Some((row, col, ixfe)) = cell_ref(body)
                    && let Some(value) = body.get(6..14)
                {
                    if value[6] == 0xFF && value[7] == 0xFF {
                        match value[0] {
                            0x00 => pending = Some((row, col, ixfe)),
                            0x01 => {
                                let text = if value[2] == 0 { "FALSE" } else { "TRUE" };
                                put(&mut out, row, col, text.to_string());
                            }
                            0x02 => {
                                if let Some(text) = error_literal(value[2]) {
                                    put(&mut out, row, col, text.to_string());
                                }
                            }
                            // 0x03 is a blank string result.
                            _ => {}
                        }
                    } else if let Some(n) = get_f64(body, 6) {
                        let text = render_number(globals.format(ixfe), n, globals.date1904);
                        put(&mut out, row, col, text);
                    }
                }
            }
            STRING => {
                let (segs, after) = continued(data, body, pos, records)?;
                pos = after;
                if let Some((row, col, ixfe)) = pending.take()
                    && let Some(mut r) = string_reader(&segs, 0)
                    && let Some(text) = globals.read_string(&mut r, false)
                {
                    put(
                        &mut out,
                        row,
                        col,
                        format_as_text(globals.format(ixfe), &clean_text(&text)),
                    );
                }
            }
            _ => {}
        }
    }
    Ok(Some(out))
}

/// Record only non-empty cell text, like the xlsx reader.
fn put(out: &mut SheetContent, row: u32, col: u32, text: String) {
    if !text.is_empty() {
        out.cells.insert((row, col), text);
    }
}

/// A reader over `segs` with the first segment's leading `skip` bytes (the
/// cell header before the string) removed.
fn string_reader<'a>(segs: &'a [&'a [u8]], skip: usize) -> Option<SegReader<'a>> {
    let mut parts: Vec<&[u8]> = Vec::with_capacity(segs.len());
    parts.push(segs.first()?.get(skip..)?);
    parts.extend(&segs[1..]);
    Some(SegReader::new(parts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CellSlot, Table, inlines_to_plain_text};
    use std::io::Write;

    fn rec(rec_type: u16, body: &[u8]) -> Vec<u8> {
        let mut out = rec_type.to_le_bytes().to_vec();
        out.extend((body.len() as u16).to_le_bytes());
        out.extend_from_slice(body);
        out
    }

    fn bof(dt: u16, vers: u16) -> Vec<u8> {
        let mut body = vec![0u8; 16];
        body[..2].copy_from_slice(&vers.to_le_bytes());
        body[2..4].copy_from_slice(&dt.to_le_bytes());
        rec(BOF, &body)
    }

    /// XLUnicodeString, compressed ASCII.
    fn ustr(s: &str) -> Vec<u8> {
        let mut out = (s.len() as u16).to_le_bytes().to_vec();
        out.push(0);
        out.extend_from_slice(s.as_bytes());
        out
    }

    /// ShortXLUnicodeString, compressed ASCII.
    fn short_ustr(s: &str) -> Vec<u8> {
        let mut out = vec![s.len() as u8, 0];
        out.extend_from_slice(s.as_bytes());
        out
    }

    fn cell6(row: u16, col: u16, ixfe: u16) -> Vec<u8> {
        [row, col, ixfe].iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn labelsst(row: u16, col: u16, ixfe: u16, isst: u32) -> Vec<u8> {
        let mut body = cell6(row, col, ixfe);
        body.extend(isst.to_le_bytes());
        rec(LABELSST, &body)
    }

    fn label(row: u16, col: u16, ixfe: u16, s: &str) -> Vec<u8> {
        let mut body = cell6(row, col, ixfe);
        body.extend(ustr(s));
        rec(LABEL, &body)
    }

    fn number(row: u16, col: u16, ixfe: u16, v: f64) -> Vec<u8> {
        let mut body = cell6(row, col, ixfe);
        body.extend(v.to_le_bytes());
        rec(NUMBER, &body)
    }

    fn rk_cell(row: u16, col: u16, ixfe: u16, rk: u32) -> Vec<u8> {
        let mut body = cell6(row, col, ixfe);
        body.extend(rk.to_le_bytes());
        rec(RK, &body)
    }

    fn formula(row: u16, col: u16, ixfe: u16, value: [u8; 8]) -> Vec<u8> {
        let mut body = cell6(row, col, ixfe);
        body.extend(value);
        body.extend([0u8; 6]);
        rec(FORMULA, &body)
    }

    fn rk_from_f64(v: f64) -> u32 {
        ((v.to_bits() >> 32) as u32) & !3
    }

    fn rk_from_int(v: i32) -> u32 {
        ((v as u32) << 2) | 2
    }

    fn ole_with(name: &str, data: &[u8]) -> Vec<u8> {
        let mut ole = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
        ole.create_stream(name).unwrap().write_all(data).unwrap();
        ole.into_inner().into_inner()
    }

    /// Assemble a BIFF8 workbook: globals plus one substream per sheet,
    /// with each BOUNDSHEET's lbPlyPos patched to the real offset.
    #[derive(Default)]
    struct Wb {
        date1904: bool,
        filepass: bool,
        /// FORMAT records: (ifmt, code).
        formats: Vec<(u16, &'static str)>,
        /// XF records in table order, each carrying its ifmt.
        xfs: Vec<u16>,
        /// Raw SST records (the SST plus any CONTINUE records).
        sst: Vec<Vec<u8>>,
        /// (name, hsState, substream records without BOF/EOF).
        sheets: Vec<(&'static str, u8, Vec<u8>)>,
    }

    impl Wb {
        fn build(&self) -> Vec<u8> {
            let mut stream = bof(0x0005, 0x0600);
            if self.filepass {
                stream.extend(rec(FILEPASS, &[0u8; 6]));
            }
            if self.date1904 {
                stream.extend(rec(DATEMODE, &1u16.to_le_bytes()));
            }
            for (ifmt, code) in &self.formats {
                let mut body = ifmt.to_le_bytes().to_vec();
                body.extend(ustr(code));
                stream.extend(rec(FORMAT, &body));
            }
            for ifmt in &self.xfs {
                let mut body = vec![0u8; 20];
                body[2..4].copy_from_slice(&ifmt.to_le_bytes());
                stream.extend(rec(XF, &body));
            }
            for raw in &self.sst {
                stream.extend(raw);
            }
            let mut patch_at = Vec::new();
            for (name, state, _) in &self.sheets {
                let mut body = vec![0u8; 4];
                body.push(*state);
                body.push(0);
                body.extend(short_ustr(name));
                patch_at.push(stream.len() + 4);
                stream.extend(rec(BOUNDSHEET, &body));
            }
            stream.extend(rec(EOF_REC, &[]));
            for (i, (_, _, records)) in self.sheets.iter().enumerate() {
                let offset = (stream.len() as u32).to_le_bytes();
                stream[patch_at[i]..patch_at[i] + 4].copy_from_slice(&offset);
                stream.extend(bof(WORKSHEET_SUBSTREAM, 0x0600));
                stream.extend_from_slice(records);
                stream.extend(rec(EOF_REC, &[]));
            }
            ole_with("Workbook", &stream)
        }
    }

    fn one_sheet(records: Vec<u8>) -> Wb {
        Wb { xfs: vec![0], sheets: vec![("S", 0, records)], ..Wb::default() }
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
    fn sst_string_split_mid_word_re_reads_the_flags_byte() {
        // Trap 1: the string continues into a CONTINUE record and switches
        // from 8-bit compressed to 16-bit encoding at the boundary, marked
        // by the repeated option-flags byte.
        let mut base = 1u32.to_le_bytes().to_vec();
        base.extend(1u32.to_le_bytes());
        base.extend(11u16.to_le_bytes());
        base.push(0x00);
        base.extend(b"HELLO");
        let mut cont = vec![0x01];
        for unit in " WORLD".encode_utf16() {
            cont.extend(unit.to_le_bytes());
        }
        let wb = Wb {
            sst: vec![rec(SST, &base), rec(CONTINUE, &cont)],
            ..one_sheet(labelsst(0, 0, 0, 0))
        };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["HELLO WORLD"]]);
    }

    #[test]
    fn rich_and_phonetic_headers_do_not_misalign_the_table() {
        // Trap 2: the optional rich-run count and phonetic size headers and
        // their trailing data must be consumed, or the next string reads
        // from the middle of them.
        let mut body = 2u32.to_le_bytes().to_vec();
        body.extend(2u32.to_le_bytes());
        body.extend(2u16.to_le_bytes());
        body.push(0x0C); // fRichSt | fExtSt
        body.extend(1u16.to_le_bytes()); // cRun
        body.extend(4u32.to_le_bytes()); // cbExtRst
        body.extend(b"ab");
        body.extend([0xAA; 4]); // the rich run
        body.extend([0xBB; 4]); // the phonetic block
        body.extend(ustr("ok"));
        let mut records = labelsst(0, 0, 0, 0);
        records.extend(labelsst(0, 1, 0, 1));
        let wb = Wb { sst: vec![rec(SST, &body)], ..one_sheet(records) };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["ab", "ok"]]);
    }

    #[test]
    fn rk_and_mulrk_encodings_decode() {
        // Trap 3: RK packs an integer or a truncated double, with a
        // divide-by-100 bit; MULRK packs a run of them, each with its own
        // format index.
        let mut records = rk_cell(0, 0, 0, rk_from_f64(1234.5));
        records.extend(rk_cell(0, 1, 0, rk_from_int(15550) | 1)); // 155.5
        let mut mulrk = cell6(0, 2, 0)[..4].to_vec(); // rw, colFirst
        mulrk.extend(0u16.to_le_bytes()); // ixfe General
        mulrk.extend(rk_from_int(7).to_le_bytes());
        mulrk.extend(1u16.to_le_bytes()); // ixfe of the percent XF
        mulrk.extend(rk_from_f64(0.5).to_le_bytes());
        mulrk.extend(3u16.to_le_bytes()); // colLast
        records.extend(rec(MULRK, &mulrk));
        let wb = Wb {
            formats: vec![(164, "0%")],
            xfs: vec![0, 164],
            sheets: vec![("S", 0, records)],
            ..Wb::default()
        };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["1234.5", "155.5", "7", "50%"]]);
    }

    #[test]
    fn number_formats_apply_to_numeric_cells() {
        // Trap 4: the cell's ixfe indexes the XF table, whose ifmt resolves
        // against FORMAT records first and the built-in table second (ifmt
        // 3 has no FORMAT record here).
        let mut records = number(0, 0, 1, 0.155);
        records.extend(number(0, 1, 2, 1234.5));
        records.extend(number(0, 2, 3, 9876543.0));
        let wb = Wb {
            formats: vec![(164, "0.0%"), (165, "\"$\"#,##0.00")],
            xfs: vec![0, 164, 165, 3],
            sheets: vec![("S", 0, records)],
            ..Wb::default()
        };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["15.5%", "$1,234.50", "9,876,543"]]);
    }

    #[test]
    fn date_serials_render_iso_in_both_date_systems() {
        let dated = |date1904| Wb {
            date1904,
            formats: vec![(164, "yyyy-mm-dd")],
            xfs: vec![164],
            sheets: vec![("S", 0, number(0, 0, 0, if date1904 { 100.0 } else { 46096.0 }))],
            ..Wb::default()
        };
        let doc = parse(&dated(false).build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["2026-03-15"]]);
        let doc = parse(&dated(true).build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["1904-04-10"]]);
    }

    #[test]
    fn hidden_rows_columns_and_sheets_are_omitted() {
        let mut records = label(0, 0, 0, "a");
        records.extend(label(0, 1, 0, "hidden col"));
        records.extend(label(0, 2, 0, "c"));
        records.extend(label(1, 0, 0, "hidden row"));
        records.extend(label(2, 0, 0, "d"));
        let mut row = vec![0u8; 16];
        row[..2].copy_from_slice(&1u16.to_le_bytes());
        row[12] = 0x20; // fDyZero
        records.extend(rec(ROW, &row));
        let mut colinfo = vec![0u8; 12];
        colinfo[..2].copy_from_slice(&1u16.to_le_bytes());
        colinfo[2..4].copy_from_slice(&1u16.to_le_bytes());
        colinfo[8] = 0x01; // fHidden
        records.extend(rec(COLINFO, &colinfo));
        let wb = Wb {
            xfs: vec![0],
            sheets: vec![("Shown", 0, records), ("Secret", 1, label(0, 0, 0, "secret"))],
            ..Wb::default()
        };
        let doc = parse(&wb.build()).unwrap();
        assert_eq!(doc.blocks.len(), 1, "hidden sheet must add no heading and no table");
        assert_eq!(texts(first_table(&doc)), vec![vec!["a", "c"], vec!["d", ""]]);
    }

    #[test]
    fn merge_extends_past_the_populated_range() {
        // Issue #8: the only populated cell anchors F1:O3, so the grid must
        // widen to the merge's full 3x10 extent.
        let mut records = label(0, 5, 0, "wide");
        let mut merged = 1u16.to_le_bytes().to_vec();
        for v in [0u16, 2, 5, 14] {
            merged.extend(v.to_le_bytes());
        }
        records.extend(rec(MERGEDCELLS, &merged));
        let doc = parse(&one_sheet(records).build()).unwrap();
        let table = first_table(&doc);
        assert_eq!(table.grid.len(), 3);
        assert_eq!(table.grid[1].len(), 10);
        let CellSlot::Origin(cell) = &table.grid[0][0] else {
            panic!("expected the merge origin at (0,0)");
        };
        assert_eq!((cell.col_span, cell.row_span), (10, 3));
    }

    #[test]
    fn formula_cached_values_render() {
        let mut records = formula(0, 0, 0, 2.5f64.to_le_bytes());
        records.extend(formula(0, 1, 0, [0x00, 0, 0, 0, 0, 0, 0xFF, 0xFF]));
        records.extend(rec(STRING, &ustr("calc")));
        records.extend(formula(0, 2, 0, [0x01, 0, 1, 0, 0, 0, 0xFF, 0xFF]));
        records.extend(formula(0, 3, 0, [0x02, 0, 0x17, 0, 0, 0, 0xFF, 0xFF]));
        let doc = parse(&one_sheet(records).build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["2.5", "calc", "TRUE", "#REF!"]]);
    }

    #[test]
    fn boolerr_renders_bool_and_error_literals() {
        let cell = |col: u16, value: u8, is_err: u8| {
            let mut body = cell6(0, col, 0);
            body.extend([value, is_err]);
            rec(BOOLERR, &body)
        };
        let mut records = cell(0, 1, 0);
        records.extend(cell(1, 0, 0));
        records.extend(cell(2, 0x07, 1));
        let doc = parse(&one_sheet(records).build()).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["TRUE", "FALSE", "#DIV/0!"]]);
    }

    #[test]
    fn filepass_means_encrypted() {
        let wb = Wb { filepass: true, ..one_sheet(label(0, 0, 0, "x")) };
        assert!(matches!(parse(&wb.build()), Err(ConvertError::Encrypted)));
    }

    #[test]
    fn biff5_byte_strings_degrade_gracefully() {
        // A BIFF5 stream in a `Book` container: one-byte-length boundsheet
        // names and codepage-encoded LABEL text, no SST.
        let mut stream = bof(0x0005, 0x0500);
        stream.extend(rec(CODEPAGE, &1252u16.to_le_bytes()));
        let mut xf = vec![0u8; 16];
        xf[2..4].copy_from_slice(&0u16.to_le_bytes());
        stream.extend(rec(XF, &xf));
        let mut boundsheet = vec![0u8; 6];
        boundsheet.extend([1, b'S']);
        let patch_at = stream.len() + 4;
        stream.extend(rec(BOUNDSHEET, &boundsheet));
        stream.extend(rec(EOF_REC, &[]));
        let offset = (stream.len() as u32).to_le_bytes();
        stream[patch_at..patch_at + 4].copy_from_slice(&offset);
        stream.extend(bof(WORKSHEET_SUBSTREAM, 0x0500));
        let mut body = cell6(0, 0, 0);
        body.extend(6u16.to_le_bytes());
        body.extend(b"l\xE9gacy"); // cp1252 e-acute
        stream.extend(rec(LABEL, &body));
        stream.extend(rec(EOF_REC, &[]));
        let doc = parse(&ole_with("Book", &stream)).unwrap();
        assert_eq!(texts(first_table(&doc)), vec![vec!["l\u{e9}gacy"]]);
    }
}
