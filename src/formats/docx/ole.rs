//! Fork (sznb): embedded OLE workbook extraction (`excel.sheet.8`).
//!
//! Old-style Word OLE charts embed a complete Excel workbook as a BIFF8
//! `Workbook` stream inside an OLE compound file (`embeddings/oleObject*.bin`).
//! The stock parser only emitted an "Embedded object" marker line — the data
//! behind every such chart in a document was lost. This module reads the CFB
//! container, extracts the `Workbook` stream, and lifts each worksheet into a
//! [`Table`] (values only, merged ranges included).

use crate::model::{Block, Cell, CellSlot, Inline, Style, Table, TableKind};

// ---------- OLE compound file (CFB) minimal reader ----------

const SIG: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];
const FREESECT: u32 = 0xFFFFFFFF;
const ENDCHECK: u32 = 0xFFFFFFFE;

struct Cfb {
    data: Vec<u8>,
    ss: usize,        // sector size
    mss: usize,       // mini sector size
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    ministream: Vec<u8>,
    entries: Vec<(String, u32, u64)>, // name, start sector, size
}

/// Bounds-checked reads: record bodies may be truncated at the stream end,
/// and untrusted data must never panic the parser.
fn u16at(d: &[u8], o: usize) -> u16 {
    if o + 2 <= d.len() {
        u16::from_le_bytes([d[o], d[o + 1]])
    } else {
        0
    }
}
fn u32at(d: &[u8], o: usize) -> u32 {
    if o + 4 <= d.len() {
        u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
    } else {
        0
    }
}

impl Cfb {
    fn open(data: &[u8]) -> Option<Cfb> {
        if data.len() < 512 || data[..8] != SIG {
            return None;
        }
        let ss = 1usize << u16at(data, 30) as usize;
        let mss = 1usize << u16at(data, 32) as usize;
        let num_fat = u32at(data, 44) as usize;
        let first_dir = u32at(data, 48);
        let cutoff = u32at(data, 56) as u64;
        let first_mini = u32at(data, 60);
        let first_difat = u32at(data, 68);
        let _num_difat = u32at(data, 72) as usize;
        let sec = |n: u32| -> usize { 512 + n as usize * ss };

        // DIFAT: 109 entries in the header, then a chain of DIFAT sectors.
        let mut difat: Vec<u32> = Vec::with_capacity(num_fat);
        for i in 0..109 {
            let v = u32at(data, 76 + i * 4);
            if v != FREESECT {
                difat.push(v);
            }
        }
        let mut cur = first_difat;
        let mut guard = 0;
        while cur != FREESECT && cur != ENDCHECK && guard < 1 << 16 {
            let base = sec(cur);
            for i in 0..(ss / 4 - 1) {
                let v = u32at(data, base + i * 4);
                if v != FREESECT {
                    difat.push(v);
                }
            }
            cur = u32at(data, base + ss - 4);
            guard += 1;
        }
        difat.truncate(num_fat.max(difat.len().min(num_fat)));

        // FAT sectors hold the sector chains.
        let mut fat: Vec<u32> = Vec::new();
        for fs in difat.iter().take(num_fat) {
            let base = sec(*fs);
            if base + ss > data.len() {
                return None;
            }
            for i in 0..(ss / 4) {
                fat.push(u32at(data, base + i * 4));
            }
        }

        let chain = |fat: &[u32], mut s: u32| -> Vec<u32> {
            let mut out = Vec::new();
            let mut guard = 0;
            while (s as usize) < fat.len() && fat[s as usize] != ENDCHECK && guard < 1 << 22 {
                out.push(s);
                s = fat[s as usize];
                guard += 1;
            }
            out
        };

        // Directory stream.
        let dir_secs = chain(&fat, first_dir);
        let mut dir = Vec::with_capacity(dir_secs.len() * ss);
        for s in &dir_secs {
            let b = sec(*s);
            if b + ss > data.len() {
                return None;
            }
            dir.extend_from_slice(&data[b..b + ss]);
        }

        let mut entries = Vec::new();
        let mut root: Option<(u32, u64)> = None;
        for e in dir.chunks(128) {
            if e.len() < 128 {
                break;
            }
            let nlen = u16at(e, 64) as usize;
            if nlen < 2 || nlen > 64 {
                continue;
            }
            // Directory entry names are UTF-16LE (nlen includes the NUL).
            let name = {
                let units: Vec<u16> = e[..nlen - 2]
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&units)
            };
            let kind = e[66];
            let start = u32at(e, 116);
            let size = u64::from(u32at(e, 120)) | (u64::from(u32at(e, 124)) << 32);
            if kind == 5 && name == "Root Entry" {
                root = Some((start, size));
            }
            if kind == 2 {
                entries.push((name, start, size));
            }
        }

        // Mini stream + mini FAT.
        let ministream = root
            .and_then(|(start, size)| {
                let secs = chain(&fat, start);
                let mut buf = Vec::with_capacity(size as usize);
                for s in secs {
                    let b = sec(s);
                    if b + ss > data.len() {
                        return None;
                    }
                    buf.extend_from_slice(&data[b..b + ss]);
                }
                buf.truncate(size as usize);
                Some(buf)
            })
            .unwrap_or_default();
        let mut mini_fat = Vec::new();
        for s in chain(&fat, first_mini) {
            let b = sec(s);
            if b + ss > data.len() {
                return None;
            }
            for i in 0..(ss / 4) {
                mini_fat.push(u32at(data, b + i * 4));
            }
        }

        let _ = cutoff;
        Some(Cfb { data: data.to_vec(), ss, mss, fat, mini_fat, ministream, entries })
    }

    fn stream(&self, want: &str) -> Option<Vec<u8>> {
        let (name, start, size) = self.entries.iter().find(|(n, _, _)| n == want)?;
        let _ = name;
        let size = *size as usize;
        if size >= 4096 {
            let mut out = Vec::with_capacity(size);
            let mut s = *start;
            let mut guard = 0;
            while (s as usize) < self.fat.len() && self.fat[s as usize] != ENDCHECK && guard < 1 << 22 {
                let b = 512 + s as usize * self.ss;
                if b + self.ss > self.data.len() {
                    return None;
                }
                out.extend_from_slice(&self.data[b..b + self.ss]);
                s = self.fat[s as usize];
                guard += 1;
            }
            out.truncate(size);
            Some(out)
        } else {
            let mut out = Vec::with_capacity(size);
            let mut s = *start;
            let mut guard = 0;
            while (s as usize) < self.mini_fat.len()
                && self.mini_fat[s as usize] != ENDCHECK
                && guard < 1 << 22
            {
                let b = s as usize * self.mss;
                if b + self.mss > self.ministream.len() {
                    return None;
                }
                out.extend_from_slice(&self.ministream[b..b + self.mss]);
                s = self.mini_fat[s as usize];
                guard += 1;
            }
            out.truncate(size);
            Some(out)
        }
    }
}

// ---------- BIFF8 minimal extraction ----------

struct Segs {
    parts: Vec<Vec<u8>>,
    seg: usize,
    pos: usize,
}

impl Segs {
    fn u8(&mut self) -> Option<u8> {
        loop {
            let p = self.parts.get(self.seg)?;
            if self.pos < p.len() {
                let b = p[self.pos];
                self.pos += 1;
                return Some(b);
            }
            self.seg += 1;
            self.pos = 0;
        }
    }
    fn u16(&mut self) -> Option<u16> {
        let lo = self.u8()?;
        let hi = self.u8()?;
        Some(u16::from_le_bytes([lo, hi]))
    }
    fn u32(&mut self) -> Option<u32> {
        let a = self.u16()?;
        let b = self.u16()?;
        Some(u32::from_le_bytes([a.to_le_bytes()[0], a.to_le_bytes()[1],
                                 b.to_le_bytes()[0], b.to_le_bytes()[1]]))
    }
    /// Read `chars` characters; at a segment boundary the next byte selects
    /// 8/16-bit for the remainder (CONTINUE semantics).
    fn xl_string(&mut self, chars: usize, wide: bool) -> Option<String> {
        let mut out = String::new();
        let mut wide = wide;
        let mut done = 0;
        while done < chars {
            // Advance past exhausted segments (get() keeps this in bounds).
            while self.parts.get(self.seg).is_some_and(|p| self.pos >= p.len()) {
                self.seg += 1;
                self.pos = 0;
            }
            if self.parts.get(self.seg).is_none() {
                return Some(out);
            }
            // A segment boundary mid-string begins with a fresh flags byte.
            if self.pos == 0 && self.seg > 0 {
                let Some(flags) = self.u8() else { return Some(out) };
                wide = flags & 1 == 1;
            }
            let ch = if wide {
                let Some(c) = self.u16() else { return Some(out) };
                char::from_u32(u32::from(c)).unwrap_or('\u{FFFD}')
            } else {
                let Some(c) = self.u8() else { return Some(out) };
                char::from_u32(u32::from(c)).unwrap_or('\u{FFFD}')
            };
            out.push(ch);
            done += 1;
        }
        Some(out)
    }
}

/// Short XLUnicodeString in a record body: cch u16, grbit u16.
fn short_string(rec: &[u8]) -> String {
    if rec.len() < 4 {
        return String::new();
    }
    let cch = u16at(rec, 0) as usize;
    let wide = rec[2] & 1 == 1;
    let bytes = if wide { rec[4..].to_vec() } else { rec[3..].to_vec() };
    let mut segs = Segs { parts: vec![bytes], seg: 0, pos: 0 };
    segs.xl_string(cch, wide).unwrap_or_default()
}

fn rk_value(v: u32) -> f64 {
    if v & 2 != 0 {
        let n = (v >> 2) as i32;
        let mut f = n as f64;
        if v & 1 != 0 {
            f /= 100.0;
        }
        f
    } else {
        let bits = (u64::from(v & 0xFFFFFFFC) << 32);
        let mut f = f64::from_bits(bits);
        if v & 1 != 0 {
            f /= 100.0;
        }
        f
    }
}

fn num_text(f: f64) -> String {
    if f == f.trunc() && f.abs() < 1e15 {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

struct SheetBuilder {
    cells: Vec<Vec<Option<String>>>, // [row][col]
    merged: Vec<(usize, usize, usize, usize)>, // r1, r2, c1, c2 inclusive
}

impl SheetBuilder {
    fn put(&mut self, r: usize, c: usize, v: String) {
        if self.cells.len() <= r {
            self.cells.resize(r + 1, Vec::new());
        }
        if self.cells[r].len() <= c {
            self.cells[r].resize(c + 1, None);
        }
        self.cells[r][c] = Some(v);
    }

    fn build(self) -> Option<Table> {
        let rows = self.cells.len();
        let cols = self.cells.iter().map(|r| r.len()).max()?; // cols == 0 -> None
        // Trim trailing all-empty rows/cols.
        let last_row = (0..rows).rev().find(|&r| self.cells[r].iter().any(|c| c.is_some()))?;
        let last_col = (0..cols).rev().find(|&c|
            self.cells.iter().any(|r| r.get(c).is_some_and(|v| v.is_some())))?;
        let covered = |r: usize, c: usize| {
            self.merged.iter().any(|&(r1, r2, c1, c2)| {
                r >= r1 && r <= r2 && c >= c1 && c <= c2 && !(r == r1 && c == c1)
            })
        };
        let span_of = |r: usize, c: usize| {
            self.merged.iter().find_map(|&(r1, r2, c1, c2)| {
                (r == r1 && c == c1).then_some((c2 - c1 + 1, r2 - r1 + 1))
            })
        };
        let mut grid: Vec<Vec<CellSlot>> = Vec::with_capacity(last_row + 1);
        let mut any = false;
        for r in 0..=last_row {
            let mut row = Vec::with_capacity(last_col + 1);
            for c in 0..=last_col {
                if covered(r, c) {
                    // Find its origin for the back-reference.
                    let org = self.merged.iter().find(|&&(r1, r2, c1, c2)| {
                        r >= r1 && r <= r2 && c >= c1 && c <= c2
                    });
                    let (or, oc) = org.map(|&(r1, _r2, c1, _c2)| (r1, c1)).unwrap_or((r, c));
                    row.push(CellSlot::Covered { origin_row: or, origin_col: oc });
                } else {
                    let text = self.cells[r].get(c).and_then(|v| v.clone()).unwrap_or_default();
                    if !text.is_empty() {
                        any = true;
                    }
                    let (col_span, row_span) = span_of(r, c).unwrap_or((1, 1));
                    row.push(CellSlot::Origin(Cell {
                        blocks: text_to_block(&text),
                        col_span: col_span as u32,
                        row_span: row_span as u32,
                    }));
                }
            }
            grid.push(row);
        }
        any.then(|| Table { grid, header_rows: 0, kind: TableKind::Data })
    }
}

fn text_to_block(text: &str) -> Vec<Block> {
    if text.is_empty() {
        return Vec::new();
    }
    vec![Block::Paragraph(vec![Inline::Text { text: text.to_string(), style: Style::PLAIN }])]
}

fn biff_tables(wb: &[u8]) -> Vec<Table> {
    // Pass 1 (workbook globals): SST + BOUNDSHEET positions.
    let mut sst: Vec<String> = Vec::new();
    let mut sheets: Vec<usize> = Vec::new();
    let mut pos = 0usize;
    let mut sst_pending: Vec<Vec<u8>> = Vec::new();
    while pos + 4 <= wb.len() {
        let t = u16at(wb, pos);
        let len = u16at(wb, pos + 2) as usize;
        let body = &wb[pos + 4..(pos + 4 + len).min(wb.len())];
        match t {
            0x0809 if pos == 0 => {} // workbook globals BOF
            0x0085 => {
                // BOUNDSHEET: lbPlyPos u32 | state u16 | sheetType u8 | name
                if body.len() >= 7 {
                    sheets.push(u32at(body, 0) as usize);
                }
            }
            0x00FC => {
                sst_pending.clear();
                sst_pending.push(body.to_vec());
            }
            0x003C if !sst_pending.is_empty() => {
                sst_pending.push(body.to_vec());
            }
            0x000A => {
                if !sst_pending.is_empty() {
                    sst = parse_sst(&sst_pending);
                    sst_pending.clear();
                }
                if !sheets.is_empty() || pos > 0 {
                    break; // end of globals
                }
            }
            _ => {}
        }
        pos += 4 + len;
    }

    let mut out = Vec::new();
    for start in sheets {
        if let Some(tb) = parse_sheet(wb, start, &sst) {
            out.push(tb);
        }
    }
    out
}

fn parse_sst(parts: &[Vec<u8>]) -> Vec<String> {
    let first = &parts[0];
    if first.len() < 8 {
        return Vec::new();
    }
    let unique = u32at(first, 4) as usize;
    // String data starts at offset 8 of the first part.
    let mut segs = Segs {
        parts: {
            let mut v: Vec<Vec<u8>> = parts.to_vec();
            v[0] = first[8..].to_vec();
            v
        },
        seg: 0,
        pos: 0,
    };
    let mut out = Vec::with_capacity(unique.min(1 << 20));
    for _ in 0..unique {
        let Some(cch) = segs.u16() else { break };
        // One flags byte, not the spec's two: WPS-ET-authored BIFF writes
        // the SST grbit as a single byte (verified byte-level on the
        // government-corpus files — a 2-byte read shifts every string).
        let Some(flags) = segs.u8() else { break };
        let wide = flags & 1 == 1;
        let rich = flags & 8 != 0;
        let far_east = flags & 4 != 0;
        let c_run = if rich { segs.u16().unwrap_or(0) as usize } else { 0 };
        let cb_ext = if far_east { segs.u32().unwrap_or(0) as usize } else { 0 };
        let s = segs.xl_string(cch as usize, wide).unwrap_or_default();
        // 4 bytes per rich run, then the Far-East string blob.
        for _ in 0..(4 * c_run + cb_ext) {
            if segs.u8().is_none() {
                break;
            }
        }
        out.push(s);
    }
    out
}

fn parse_sheet(wb: &[u8], start: usize, sst: &[String]) -> Option<Table> {
    let mut b = SheetBuilder { cells: Vec::new(), merged: Vec::new() };
    // A FORMULA whose result is a string is followed by a STRING record
    // carrying the value for that (row, col).
    let mut pending_string_formula: Option<(usize, usize)> = None;
    let mut pos = start;
    while pos + 4 <= wb.len() {
        let t = u16at(wb, pos);
        let len = u16at(wb, pos + 2) as usize;
        let body = &wb[pos + 4..(pos + 4 + len).min(wb.len())];
        match t {
            0x00FD => if body.len() >= 10 { // LABELSST: row, col, ixfe, isst
                let r = u16at(body, 0) as usize;
                let c = u16at(body, 2) as usize;
                let idx = u32at(body, 6) as usize;
                if let Some(s) = sst.get(idx) {
                    b.put(r, c, s.clone());
                }
            },
            0x0203 => if body.len() >= 14 { // NUMBER
                let r = u16at(body, 0) as usize;
                let c = u16at(body, 2) as usize;
                let mut b8 = [0u8; 8];
                b8.copy_from_slice(&body[6..14]);
                b.put(r, c, num_text(f64::from_le_bytes(b8)));
            },
            0x027E => if body.len() >= 10 { // RK
                let r = u16at(body, 0) as usize;
                let c = u16at(body, 2) as usize;
                b.put(r, c, num_text(rk_value(u32at(body, 6))));
            },
            0x00BD => { // MULRK. Standard: row rowLast colFirst {IXFE,RK}* colLast
                // (len = N*6+8). WPS-ET variant omits colFirst entirely
                // (len = N*6+6); its first column is colLast-N+1 — verified
                // byte-level on the government corpus (row 1: cols 1..3,
                // 56.67 / -14.4 / 0.4).
                let blen = body.len();
                if blen >= 12 && blen % 6 == 0 {
                    let n = (blen - 6) / 6;
                    let r = u16at(body, 0) as usize;
                    let c2 = u16at(body, blen - 2) as usize;
                    if c2 + 1 >= n {
                        let c1 = c2 + 1 - n;
                        for i in 0..n {
                            let off = 4 + i * 6;
                            b.put(r, c1 + i, num_text(rk_value(u32at(body, off + 2))));
                        }
                    }
                } else if blen >= 8 {
                    let r = u16at(body, 0) as usize;
                    let c1 = u16at(body, 4) as usize;
                    let c2 = u16at(body, blen - 2) as usize;
                    for c in c1..=c2 {
                        let off = 6 + (c - c1) * 6;
                        if off + 6 > blen {
                            break;
                        }
                        b.put(r, c, num_text(rk_value(u32at(body, off + 2))));
                    }
                }
            },
            0x0006 => if body.len() >= 14 { // FORMULA
                let r = u16at(body, 0) as usize;
                let c = u16at(body, 2) as usize;
                let mut b8 = [0u8; 8];
                b8.copy_from_slice(&body[6..14]);
                let f = f64::from_le_bytes(b8);
                if f.is_finite() {
                    b.put(r, c, num_text(f));
                } else {
                    // Non-numeric result: the value follows in a STRING record.
                    pending_string_formula = Some((r, c));
                }
            },
            0x0207 => { // STRING: value of the preceding string-result FORMULA
                if let Some((r, c)) = pending_string_formula.take()
                    && body.len() >= 3
                {
                    let cch = u16at(body, 0) as usize;
                    let wide = body[2] & 1 == 1;
                    let data = &body[3..];
                    let mut segs = Segs { parts: vec![data.to_vec()], seg: 0, pos: 0 };
                    b.put(r, c, segs.xl_string(cch, wide).unwrap_or_default());
                }
            },
            0x0204 | 0x00D6 => if body.len() >= 6 { // LABEL / RSTRING
                let r = u16at(body, 0) as usize;
                let c = u16at(body, 2) as usize;
                b.put(r, c, short_string(&body[6..]));
            },
            0x00E5 => { // MERGEDCELLS
                if body.len() >= 2 {
                    let n = u16at(body, 0) as usize;
                    for i in 0..n {
                        let o = 2 + i * 8;
                        if o + 8 <= body.len() {
                            b.merged.push((
                                u16at(body, o) as usize,
                                u16at(body, o + 2) as usize,
                                u16at(body, o + 4) as usize,
                                u16at(body, o + 6) as usize,
                            ));
                        }
                    }
                }
            }
            0x000A => break, // EOF: sheet done
            _ => {}
        }
        pos += 4 + len;
    }
    b.build()
}

/// Extract worksheet tables from an `excel.sheet.8` OLE payload.
pub fn workbook_tables(bytes: &[u8]) -> Option<Vec<Table>> {
    let cfb = Cfb::open(bytes)?;
    let wb = cfb.stream("Workbook")?;
    let tables = biff_tables(&wb);
    (!tables.is_empty()).then_some(tables)
}
